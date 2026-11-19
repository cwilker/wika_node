import { test } from 'node:test';
import assert from 'node:assert/strict';
import { extractHeadProof } from '../src/proof-html.mjs';
const proof = 'wika-claim-v1:opaque-envelope';
const tag = `<meta name="wika-claim" content="${proof}">`;
const page = head => `<!doctype html><html><head>${head}</head><body>Article</body></html>`;
test('extracts exactly one server-head tag without depending on unrelated page content', () => {
  for (const html of [page(tag), page(`<title>Random content</title>${tag}`), page(`<META CONTENT='${proof}' NAME='WIKA-CLAIM'>`), page(tag).replace('Article', 'Other article')]) {
    assert.deepEqual(extractHeadProof(html), { outcome: 'extracted', proof });
  }
});
for (const [name, html] of Object.entries({
  body: page('').replace('Article', tag),
  footer: page('').replace('Article', `<footer>${tag}</footer>`),
  comment: page(`<!--${tag}-->`),
  script: page(`<script>const text = '${tag}'</script>`),
  template: page(`<template>${tag}</template>`),
  noscript: page(`<noscript>${tag}</noscript>`),
  genericMarker: page('wika.network/author/bob'),
  duplicate: page(`${tag}${tag}`),
  conflict: page(`${tag}<meta name="wika-claim" content="different">`),
  duplicateAttribute: page('<meta name="wika-claim" content="a" content="b">'),
  missingContent: page('<meta name="wika-claim">'),
  impliedHead: tag,
  unclosedHead: `<html><head>${tag}<body>Article</body></html>`,
  afterHead: `<html><head></head>${tag}<body>Article</body></html>`,
  tooLong: page(`<meta name="wika-claim" content="${'a'.repeat(2049)}">`),
  oversizedHtml: page(tag + 'a'.repeat(2 * 1024 * 1024)),
  brokenComment: page(`<!-- ${tag}`),
})) test(`rejects ${name}`, () => assert.equal(extractHeadProof(html).outcome, 'invalid'));

test('visitor body markup cannot override a valid head proof', () => {
  assert.deepEqual(extractHeadProof(page(tag).replace('Article', '<meta name="wika-claim" content="attacker">')), { outcome: 'extracted', proof });
});

test('malformed unrelated body attributes do not invalidate the head proof', () => {
  assert.deepEqual(extractHeadProof(page(tag).replace('Article', '<div class="a" class="b">Content update</div>')), { outcome: 'extracted', proof });
});
