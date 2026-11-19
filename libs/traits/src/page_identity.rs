//! Page identity v1. Deliberately avoids browser URL serialization in consensus.
//! Inputs must already be ASCII URIs (Unicode hostnames use A-labels, other text uses
//! percent encoding). Unsupported/ambiguous syntax is rejected, never rewritten silently.
use alloc::{string::String, vec::Vec};

pub const VERSION: u8 = 1;
pub const MAX_URL_BYTES: usize = 254;

pub fn canonicalize(input: &[u8]) -> Option<Vec<u8>> {
	// Check the whole input, including the discarded fragment, before allocating.
	if input.is_empty()
		|| input.len() > MAX_URL_BYTES
		|| input
			.iter()
			.any(|b| !b.is_ascii() || *b <= 32 || *b >= 127 || b"\\\"<>'`{}|^[]".contains(b))
	{
		return None;
	}
	let input = core::str::from_utf8(input).ok()?;
	let raw = input.split('#').next()?;
	let (scheme, rest) = raw.split_once("://")?;
	let scheme = scheme.to_ascii_lowercase();
	if scheme != "https" && scheme != "http" {
		return None;
	}
	let end = rest.find(['/', '?']).unwrap_or(rest.len());
	let authority = &rest[..end];
	if authority.contains('@') {
		return None;
	}
	let (host, port) = match authority.split_once(':') {
		Some((host, port)) => {
			if port.is_empty() || port.starts_with('0') || !port.bytes().all(|c| c.is_ascii_digit())
			{
				return None;
			}
			let number: u16 = port.parse().ok()?;
			if number == 0 {
				return None;
			}
			(host, Some(number))
		}
		None => (authority, None),
	};
	if host.is_empty() || host.len() > 253 || host.ends_with('.') {
		return None;
	}
	let labels: Vec<_> = host.split('.').collect();
	if labels.iter().any(|label| {
		label.is_empty()
			|| label.len() > 63
			|| label.starts_with('-')
			|| label.ends_with('-')
			|| !label
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
	}) {
		return None;
	}
	if labels
		.last()?
		.to_ascii_lowercase()
		.strip_prefix("0x")
		.is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit()))
	{
		return None;
	}
	// Avoid alternate IPv4 spellings understood differently by URL/network stacks.
	if labels.last()?.bytes().all(|b| b.is_ascii_digit()) {
		if labels.len() != 4
			|| labels
				.iter()
				.any(|s| s.len() > 1 && s.starts_with('0') || s.parse::<u8>().is_err())
		{
			return None;
		}
	}
	let tail = &rest[end..];
	let (path, query) = tail
		.split_once('?')
		.map_or((tail, None), |(p, q)| (p, Some(q)));
	let mut i = 0;
	let bytes = tail.as_bytes();
	while i < bytes.len() {
		if bytes[i] == b'%' {
			if i + 2 >= bytes.len()
				|| !bytes[i + 1].is_ascii_hexdigit()
				|| !bytes[i + 2].is_ascii_hexdigit()
			{
				return None;
			}
			i += 2;
		}
		i += 1;
	}
	let mut segments = Vec::new();
	if !path.is_empty() {
		let pieces: Vec<_> = path[1..].split('/').collect();
		for (i, segment) in pieces.iter().enumerate() {
			// Encoded dot segments are normalized by some browser stacks. Reject them.
			let decoded_dot = segment.to_ascii_lowercase().replace("%2e", ".");
			if (*segment != decoded_dot) && (decoded_dot == "." || decoded_dot == "..") {
				return None;
			}
			match *segment {
				"." => {}
				".." => {
					segments.pop();
				}
				other => segments.push(other),
			}
			if i + 1 == pieces.len() && (*segment == "." || *segment == "..") {
				segments.push("");
			}
		}
	}
	let mut out = String::new();
	out.push_str(&scheme);
	out.push_str("://");
	out.push_str(&host.to_ascii_lowercase());
	if let Some(port) = port {
		if !(scheme == "https" && port == 443 || scheme == "http" && port == 80) {
			use alloc::string::ToString;
			out.push(':');
			out.push_str(&port.to_string());
		}
	}
	out.push('/');
	out.push_str(&segments.join("/"));
	if let Some(query) = query {
		out.push('?');
		out.push_str(query);
	}
	(out.len() <= MAX_URL_BYTES).then(|| out.into_bytes())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn shared_identity_corpus() {
		let fixtures: serde_json::Value = serde_json::from_str(include_str!(
			"../../../compatibility/fixtures/page-identity-v1.json"
		))
		.unwrap();
		for case in fixtures["cases"].as_array().unwrap() {
			let input = case["input"].as_str().unwrap();
			let actual = canonicalize(input.as_bytes()).map(|v| String::from_utf8(v).unwrap());
			assert_eq!(
				actual.as_deref(),
				case["expected"].as_str(),
				"input: {input}"
			);
			if let Some(actual) = actual {
				assert_eq!(canonicalize(actual.as_bytes()), Some(actual.into_bytes()));
			}
		}
	}
}
