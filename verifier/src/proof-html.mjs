import { parse } from 'parse5';
import { FETCH_LIMITS } from './fetch.mjs';
export const MAX_PROOF_BYTES = 2048;

/** Extract an opaque envelope. Signature/request validation is a separate required step.
 * Parse the server response only. Never inspect a rendered DOM or execute scripts.
 */
export function extractHeadProof(html) {
  if (typeof html !== 'string' || Buffer.byteLength(html) > FETCH_LIMITS.maxBytes) return { outcome: 'invalid', reason: 'HTML size limit' };
  const errors = [];
  const document = parse(html, { sourceCodeLocationInfo: true, onParseError: e => errors.push(e) });
  const root = document.childNodes.find(n => n.tagName === 'html');
  const head = root?.childNodes.find(n => n.tagName === 'head');
  const location = head?.sourceCodeLocation;
  // Implied head/body placement is too ambiguous for ownership proofs.
  if (!location?.startTag || !location?.endTag) return { outcome: 'invalid', reason: 'Explicit HTML head required' };
  if (errors.some(e => e.code !== 'missing-doctype' && e.startOffset <= location.endTag.endOffset)) return { outcome: 'invalid', reason: 'Malformed HTML head' };
  const proofs = [];
  for (const node of head.childNodes) {
    if (node.tagName !== 'meta') continue;
    const name = node.attrs.find(a => a.name === 'name')?.value;
    if (name?.toLowerCase() !== 'wika-claim') continue;
    const source = node.sourceCodeLocation;
    if (!source || source.startOffset < location.startTag.endOffset || source.endOffset > location.endTag.startOffset) return { outcome: 'invalid', reason: 'Proof outside explicit head' };
    const content = node.attrs.find(a => a.name === 'content')?.value;
    if (!content || content.length > MAX_PROOF_BYTES || /[^\x21-\x7e]/.test(content)) return { outcome: 'invalid', reason: 'Invalid proof encoding' };
    proofs.push(content);
  }
  if (proofs.length !== 1) return { outcome: 'invalid', reason: 'Exactly one head proof required' };
  return { outcome: 'extracted', proof: proofs[0] };
}
