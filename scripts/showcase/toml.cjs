// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// A small TOML reader for the showcase generator (#154).
//
// The generator has to read `support/features.toml` and `support/matrix.toml`
// from Node, and `ui/browser` deliberately depends on Playwright and nothing
// else. Pulling a parser from npm for two data files would add a supply-chain
// input to the one page whose whole argument is that nothing on it is taken
// on trust, so this reads the subset those files use and REFUSES the rest:
//
//   read     tables, arrays of tables, dotted and quoted keys, basic and
//            literal strings (single- and multi-line), integers, floats,
//            booleans, arrays (multi-line, trailing comma), inline tables,
//            comments
//   refused  dates and times, and anything else it does not recognise --
//            with the line number, never by guessing
//
// A value that cannot be read is an error, not `undefined`: a claim that
// silently vanished from the page is the failure this file must not have.

'use strict';

class TomlError extends Error {}

function parse(text, name = 'toml') {
  const src = text.replace(/\r\n/g, '\n');
  let i = 0;
  const root = {};
  let table = root;
  // Tables that were created by a [[header]], so a later [header] of the
  // same name is refused rather than merged into the last element.
  const arrays = new WeakSet();
  const defined = new WeakSet();

  const line = () => src.slice(0, i).split('\n').length;
  const fail = msg => { throw new TomlError(`${name}:${line()}: ${msg}`); };
  const peek = () => src[i];
  const skipWs = () => { while (src[i] === ' ' || src[i] === '\t') i++; };
  const skipComment = () => { if (src[i] === '#') while (i < src.length && src[i] !== '\n') i++; };
  const skipBlank = () => {
    for (;;) {
      skipWs();
      skipComment();
      if (src[i] === '\n') i++; else break;
    }
  };
  const endOfLine = () => {
    skipWs();
    skipComment();
    if (i < src.length && src[i] !== '\n') fail(`unexpected ${JSON.stringify(src[i])} after the value`);
  };

  function escape() {
    const c = src[i++];
    const simple = {b: '\b', t: '\t', n: '\n', f: '\f', r: '\r', '"': '"', '\\': '\\'};
    if (c in simple) return simple[c];
    if (c === 'u' || c === 'U') {
      const len = c === 'u' ? 4 : 8;
      const hex = src.slice(i, i + len);
      if (!/^[0-9a-fA-F]+$/.test(hex) || hex.length !== len) fail('bad unicode escape');
      i += len;
      return String.fromCodePoint(parseInt(hex, 16));
    }
    return fail(`unknown escape \\${c}`);
  }

  function string() {
    const q = src[i];
    const multi = src.startsWith(q.repeat(3), i);
    i += multi ? 3 : 1;
    if (multi && src[i] === '\n') i++;
    let out = '';
    for (;;) {
      if (i >= src.length) fail('unterminated string');
      if (multi && src.startsWith(q.repeat(3), i)) {
        // up to two quotes may sit directly before the closing delimiter
        let extra = 0;
        while (src[i + 3 + extra] === q && extra < 2) extra++;
        out += q.repeat(extra);
        i += 3 + extra;
        return out;
      }
      const c = src[i];
      if (!multi && c === q) { i++; return out; }
      if (!multi && c === '\n') fail('a single-line string may not contain a newline');
      if (q === '"' && c === '\\') {
        i++;
        if (multi && /^[ \t]*\n/.test(src.slice(i, i + 200))) {
          // line-ending backslash: trim the newline and the indentation after it
          while (/\s/.test(src[i])) i++;
          continue;
        }
        out += escape();
        continue;
      }
      out += c;
      i++;
    }
  }

  function key() {
    const parts = [];
    for (;;) {
      skipWs();
      if (src[i] === '"' || src[i] === "'") parts.push(string());
      else {
        const m = /^[A-Za-z0-9_-]+/.exec(src.slice(i, i + 200));
        if (!m) fail('expected a key');
        parts.push(m[0]);
        i += m[0].length;
      }
      skipWs();
      if (src[i] === '.') { i++; continue; }
      return parts;
    }
  }

  function value() {
    skipWs();
    const c = peek();
    if (c === '"' || c === "'") return string();
    if (c === '[') {
      i++;
      const arr = [];
      for (;;) {
        skipBlank();
        if (src[i] === ']') { i++; return arr; }
        arr.push(value());
        skipBlank();
        if (src[i] === ',') { i++; continue; }
        if (src[i] === ']') { i++; return arr; }
        fail('expected , or ] in an array');
      }
    }
    if (c === '{') {
      i++;
      const obj = {};
      skipWs();
      if (src[i] === '}') { i++; return obj; }
      for (;;) {
        const k = key();
        if (src[i] !== '=') fail('expected = in an inline table');
        i++;
        assign(obj, k, value());
        skipWs();
        if (src[i] === ',') { i++; continue; }
        if (src[i] === '}') { i++; return obj; }
        fail('expected , or } in an inline table');
      }
    }
    const m = /^[^\s,\]}#]+/.exec(src.slice(i, i + 200));
    if (!m) fail('expected a value');
    const tok = m[0];
    i += tok.length;
    if (tok === 'true') return true;
    if (tok === 'false') return false;
    if (/^[+-]?(0|[1-9](_?\d)*)$/.test(tok)) return parseInt(tok.replace(/_/g, ''), 10);
    if (/^0x[0-9a-fA-F_]+$/.test(tok)) return parseInt(tok.slice(2).replace(/_/g, ''), 16);
    if (/^[+-]?(0|[1-9](_?\d)*)(\.\d(_?\d)*)?([eE][+-]?\d+)?$/.test(tok)) return parseFloat(tok.replace(/_/g, ''));
    return fail(`unsupported value ${JSON.stringify(tok)} (dates, times and special floats are not read)`);
  }

  const setOwn = (obj, k, v) => Object.defineProperty(obj, k, {value: v, enumerable: true, writable: true, configurable: true});

  function descend(obj, parts) {
    for (const p of parts) {
      if (!Object.hasOwn(obj, p)) setOwn(obj, p, {});
      let next = obj[p];
      if (Array.isArray(next)) {
        if (!arrays.has(next)) fail(`${p} is a value array, not an array of tables`);
        next = next[next.length - 1];
      } else if (typeof next !== 'object' || next === null) {
        fail(`${p} is already a value`);
      }
      obj = next;
    }
    return obj;
  }

  function assign(obj, parts, v) {
    const last = parts[parts.length - 1];
    const owner = descend(obj, parts.slice(0, -1));
    if (Object.hasOwn(owner, last)) fail(`duplicate key ${parts.join('.')}`);
    setOwn(owner, last, v);
  }

  for (;;) {
    skipBlank();
    if (i >= src.length) break;
    if (src[i] === '[') {
      const isArray = src[i + 1] === '[';
      i += isArray ? 2 : 1;
      const parts = key();
      if (isArray) {
        if (src[i] !== ']' || src[i + 1] !== ']') fail('expected ]]');
        i += 2;
        const owner = descend(root, parts.slice(0, -1));
        const last = parts[parts.length - 1];
        if (!Object.hasOwn(owner, last)) {
          const arr = [];
          setOwn(owner, last, arr);
          arrays.add(arr);
        }
        if (!Array.isArray(owner[last]) || !arrays.has(owner[last])) fail(`${parts.join('.')} is not an array of tables`);
        table = {};
        owner[last].push(table);
        // An element of an array of tables IS a defined table. It was not
        // marked as one, so the refusal below never fired: `[feature.shot]`
        // written with single brackets after a `[[feature.shot]]` merged
        // silently into the last element and one screenshot disappeared
        // (#212 S19).
        defined.add(table);
      } else {
        if (src[i] !== ']') fail('expected ]');
        i++;
        const owner = descend(root, parts.slice(0, -1));
        const last = parts[parts.length - 1];
        if (Object.hasOwn(owner, last) && Array.isArray(owner[last]) && arrays.has(owner[last])) {
          fail(`[${parts.join('.')}] is an array of tables, declared with [[${parts.join('.')}]]: a single-bracket header would merge into its last element`);
        }
        table = descend(owner, [last]);
        if (defined.has(table)) fail(`table [${parts.join('.')}] is defined twice`);
        defined.add(table);
      }
      endOfLine();
      continue;
    }
    const k = key();
    if (src[i] !== '=') fail('expected =');
    i++;
    assign(table, k, value());
    endOfLine();
  }
  return root;
}

module.exports = {parse, TomlError};
