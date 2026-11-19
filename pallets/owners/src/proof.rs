//! Signed head-metadata envelope v1, used by the safe claim extrinsic.
//! Initial expiry gates activation, not subsequent checks of an already accepted digest.
use alloc::vec::Vec;
use codec::{Decode, DecodeAll, Encode};
use frame_support::{traits::ConstU32, BoundedVec};
use sp_runtime::{traits::Verify, AccountId32, MultiSignature};

pub const DOMAIN: &[u8] = b"wika/page-claim/v1\0";
pub const PREFIX: &[u8] = b"wika-claim-v1:";
pub const MAX_PROOF_BYTES: usize = 2048;

#[derive(Encode, Decode, Clone, PartialEq, Eq, Debug)]
pub struct Envelope {
	pub version: u8,
	pub method: u8,
	pub genesis: [u8; 32],
	pub page: BoundedVec<u8, ConstU32<254>>,
	pub account: AccountId32,
	pub nonce: [u8; 32],
	/// Unix milliseconds, chain timestamp (not wall clock on a client).
	pub expires_at: u64,
}
impl Envelope {
	pub fn message(&self) -> Vec<u8> {
		let mut message = DOMAIN.to_vec();
		message.extend(self.encode());
		message
	}
	pub fn supported(&self) -> bool {
		self.version == 1 && self.method == 1 && self.expires_at > 0 && self.page.starts_with(b"https://") &&
			wika_traits::page_identity::canonicalize(&self.page).is_some_and(|p| p == self.page.as_slice()) &&
			// Identity may allow nonstandard ports; this proof method does not.
			!self.page[8..].split(|b| *b == b'/' || *b == b'?').next().unwrap_or_default().contains(&b':')
	}
}

fn unhex(value: &[u8]) -> Option<Vec<u8>> {
	if value.len() % 2 != 0 {
		return None;
	}
	value
		.chunks(2)
		.map(|c| {
			fn digit(b: u8) -> Option<u8> {
				match b {
					b'0'..=b'9' => Some(b - b'0'),
					b'a'..=b'f' => Some(b - b'a' + 10),
					_ => None,
				}
			}
			Some(digit(c[0])? * 16 + digit(c[1])?)
		})
		.collect()
}

/// Decode canonical lowercase hex, verify the wallet signature, reject trailing SCALE bytes.
/// The caller must also bind genesis/page/account/nonce and enforce initial expiry.
pub fn decode_signed(content: &[u8]) -> Option<Envelope> {
	if content.len() > MAX_PROOF_BYTES {
		return None;
	}
	let mut parts = content.strip_prefix(PREFIX)?.split(|b| *b == b'.');
	let body = unhex(parts.next()?)?;
	let signature = unhex(parts.next()?)?;
	if parts.next().is_some() {
		return None;
	}
	let envelope = Envelope::decode_all(&mut &body[..]).ok()?;
	let signature = MultiSignature::decode_all(&mut &signature[..]).ok()?;
	if !envelope.supported() {
		return None;
	}
	let message = envelope.message();
	// Browser wallets may use Substrate's standard <Bytes> message wrapping.
	let mut wrapped = b"<Bytes>".to_vec();
	wrapped.extend(&message);
	wrapped.extend(b"</Bytes>");
	(signature.verify(message.as_slice(), &envelope.account)
		|| signature.verify(wrapped.as_slice(), &envelope.account))
	.then_some(envelope)
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_core::{sr25519, Pair};
	fn encode(envelope: &Envelope, signer: &sr25519::Pair, wrap: bool) -> Vec<u8> {
		let mut message = envelope.message();
		if wrap {
			let mut wrapped = b"<Bytes>".to_vec();
			wrapped.extend(message);
			wrapped.extend(b"</Bytes>");
			message = wrapped;
		}
		let signature = MultiSignature::Sr25519(signer.sign(&message));
		let mut content = PREFIX.to_vec();
		content.extend(crate::to_hex(&envelope.encode()));
		content.push(b'.');
		content.extend(crate::to_hex(&signature.encode()));
		content
	}
	#[test]
	fn client_generated_fixtures_decode_and_verify_in_runtime() {
		let data: serde_json::Value = serde_json::from_str(include_str!(
			"../../../compatibility/fixtures/claim-proof-v1.json"
		))
		.unwrap();
		for case in data["cases"].as_array().unwrap() {
			let content = case["content"].as_str().unwrap().as_bytes();
			let envelope = decode_signed(content).expect("client proof verifies in Rust");
			assert_eq!(
				&envelope.page[..],
				case["page"].as_str().unwrap().as_bytes()
			);
			assert_eq!(
				envelope.expires_at.to_string(),
				case["expiresAt"].as_str().unwrap()
			);
			assert_eq!(
				crate::to_hex(&envelope.encode()),
				case["body"].as_str().unwrap()[2..].as_bytes()
			);
			assert_eq!(
				crate::to_hex(&envelope.message()),
				case["message"].as_str().unwrap()[2..].as_bytes()
			);
			assert_eq!(
				crate::to_hex(&sp_io::hashing::blake2_256(content)),
				case["proofDigest"].as_str().unwrap()[2..].as_bytes()
			);
		}
	}

	#[test]
	fn signatures_and_bindings_are_exact() {
		let pair = sr25519::Pair::from_string("//Alice", None).unwrap();
		let mut envelope = Envelope {
			version: 1,
			method: 1,
			genesis: [1; 32],
			page: b"https://example.com/post".to_vec().try_into().unwrap(),
			account: AccountId32::from(pair.public().0),
			nonce: [2; 32],
			expires_at: 123456,
		};
		for wrap in [false, true] {
			let proof = encode(&envelope, &pair, wrap);
			assert_eq!(decode_signed(&proof), Some(envelope.clone()));
			let mut bad = proof.clone();
			bad.push(b'0');
			assert!(decode_signed(&bad).is_none());
			let mut bad = proof.clone();
			bad[20] = if bad[20] == b'0' { b'1' } else { b'0' };
			assert!(decode_signed(&bad).is_none());
		}
		let bob = sr25519::Pair::from_string("//Bob", None).unwrap();
		assert!(decode_signed(&encode(&envelope, &bob, false)).is_none());
		envelope.version = 2;
		assert!(decode_signed(&encode(&envelope, &pair, false)).is_none());
		envelope.version = 1;
		envelope.page = b"https://example.com/post#fragment"
			.to_vec()
			.try_into()
			.unwrap();
		assert!(decode_signed(&encode(&envelope, &pair, false)).is_none());
	}
}
