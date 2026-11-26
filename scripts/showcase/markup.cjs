// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The little markup support/*.toml text may use, rendered to HTML. Shared by
// the showcase page (build.cjs) and the matrix blocks in the README and the
// manual (matrix-docs.cjs), so the same sentence cannot read differently in
// two places (#153).

'use strict';

// Where the project is now: what a capture made from here on names its
// commits in (#7).
const REPO = 'https://github.com/Aravindargutus/butterfly';

// Where it was before the move. Its history was not carried over, so three
// things exist only there and are linked there: the issues the prose cites
// by number (#151, #231, ...), the commits of captures made before the
// move, and the DO-330 assessment, at the commit the README cites.
const HISTORICAL_REPO = 'https://github.com/dhaksh64/butterfly';
const ASSESSMENT_URL = `${HISTORICAL_REPO}/blob/8104609a78c0e629db0b338c36bdbd3b9f33b5c4/docs/qualification/DO-330-assessment.md`;

/// The repository a captured commit is in: the one the capture recorded,
/// and the old one for a capture that recorded none (it predates the move).
const commitUrl = (commit, repo) => `${repo || HISTORICAL_REPO}/commit/${commit}`;

const esc = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

/// The little markup a claim may use: `code`, and #123 for an issue.
function prose(text) {
  return esc(text)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/(^|[\s(])#(\d{2,4})\b/g, `$1<a href="${HISTORICAL_REPO}/issues/$2">#$2</a>`);
}

const issue = n => `<a href="${HISTORICAL_REPO}/issues/${n}">#${n}</a>`;

module.exports = {REPO, HISTORICAL_REPO, ASSESSMENT_URL, commitUrl, esc, prose, issue};
