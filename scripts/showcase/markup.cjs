// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The little markup support/*.toml text may use, rendered to HTML. Shared by
// the showcase page (build.cjs) and the matrix blocks in the README and the
// manual (matrix-docs.cjs), so the same sentence cannot read differently in
// two places (#153).

'use strict';

const REPO = 'https://github.com/dhaksh64/butterfly';

const esc = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

/// The little markup a claim may use: `code`, and #123 for an issue.
function prose(text) {
  return esc(text)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/(^|[\s(])#(\d{2,4})\b/g, `$1<a href="${REPO}/issues/$2">#$2</a>`);
}

const issue = n => `<a href="${REPO}/issues/${n}">#${n}</a>`;

module.exports = {REPO, esc, prose, issue};
