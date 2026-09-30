// Native Bun/JSC RegExp admission corpus. No ported policy or regex emulation.
import * as fs from 'node:fs';
import * as path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const manifestPath = path.resolve(process.argv[2]);
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
if (Bun.version !== '1.4.0' || manifest.runtime.version !== Bun.version) throw new Error('wrong Bun version');
if (digest(fs.readFileSync(process.execPath)) !== manifest.runtime.executableSha256) throw new Error('runtime hash mismatch');
if (digest(fs.readFileSync(fileURLToPath(import.meta.url))) !== manifest.runnerSha256) throw new Error('runner hash mismatch');

const units = value => Array.from({length:value.length}, (_, i) => value.charCodeAt(i));
const string = value => String.fromCharCode(...value);
const asUnits = value => typeof value === 'string' ? units(value) : value;
const capture = value => value === undefined ? {status:'undefined'} : {status:'string', units:units(value)};
const groups = (value, convert) => value === undefined ? null : Object.fromEntries(Object.entries(value).map(([k,v]) => [k,convert(v)]));
const range = value => value === undefined ? null : [...value];
const cases = [];
function add(id, family, source, flags, steps = []) {
  const input = {id, family, sourceUnits:asUnits(source), flags,
    steps:steps.map(step => typeof step === 'string' || Array.isArray(step)
      ? {inputUnits:asUnits(step)} : {...step, inputUnits:asUnits(step.inputUnits)})};
  let regexp;
  try { regexp = new RegExp(string(input.sourceUnits), flags); }
  catch (error) { cases.push({...input, expected:{status:'constructor-error', name:error.name, message:error.message}}); return; }
  const expected = {status:'ok', sourceUnits:units(regexp.source), flags:regexp.flags, initialLastIndex:regexp.lastIndex, steps:[]};
  for (const step of input.steps) {
    if (Object.hasOwn(step, 'setLastIndex')) regexp.lastIndex = step.setLastIndex;
    try {
      const match = regexp.exec(string(step.inputUnits));
      expected.steps.push(match === null ? {status:'no-match', lastIndex:regexp.lastIndex}
        : {status:'match', lastIndex:regexp.lastIndex, match:{wholeUnits:units(match[0]),
          captures:match.slice(1).map(capture), index:match.index, groups:groups(match.groups, capture),
          indices:match.indices === undefined ? null : {ranges:match.indices.map(range), groups:groups(match.indices.groups, range)}}});
    } catch (error) { expected.steps.push({status:'error', name:error.name, message:error.message, lastIndex:regexp.lastIndex}); }
  }
  cases.push({...input, expected});
}

for (const [id, pattern] of [['empty',''],['slash','/'],['escaped-slash','\\/'],['class-slash','[/]'],
  ['lf','\n'],['cr','\r'],['ls','\u2028'],['ps','\u2029'],['escaped-lf','\\n'],['backslash','\\\\']]) {
  add(`source-${id}`, 'source', pattern, '', [pattern]);
}
add('flags-canonical', 'flags', '.', 'ysmigdu', ['a']);
add('flags-canonical-v', 'flags', '.', 'ysmgdiv', ['a']);
for (const flags of ['gg','ii','uv','vu','z','I','dgyd']) add(`flags-invalid-${flags}`, 'invalid', '.', flags);
for (const [id,pattern,flags] of [['class','[',''],['group','(',''],['range','[z-a]',''],
  ['escape','\\',''],['property','\\p{NotAProperty}','u'],['unicode-identity','\\q','u'],
  ['v-reserved','[()]','v'],['duplicate-name','(?<a>x)(?<a>y)','']]) add(`syntax-${id}`, 'invalid', pattern, flags);

for (const [id,pattern,input] of [['long-s','s','ſ'],['kelvin','k','K'],['sigma','σ','ςΣ'],
  ['omega','ω','ΩΩ'],['micro','μ','µΜ'],['turkish','i','İıI']]) {
  for (const flags of ['ig','igu','igv']) add(`fold-${id}-${flags}`, 'fold', pattern, flags, [input,input,input,input]);
}
for (const [id,input] of [['lf','\n'],['cr','\r'],['crlf','\r\n'],['ls','\u2028'],['ps','\u2029']]) {
  add(`dot-${id}`, 'line', '.', 'g', [input,input]);
  add(`dotall-${id}`, 'line', '.', 'sg', [input,input,input]);
  add(`anchors-${id}`, 'line', '^|$', 'gm', [{inputUnits:`a${input}b`,setLastIndex:1},
    {inputUnits:`a${input}b`,setLastIndex:2},{inputUnits:`a${input}b`,setLastIndex:3}]);
  add(`end-${id}`, 'line', 'a$', '', [`a${input}`]);
}
add('anchors-basic', 'line', '^b$', 'm', ['a\r\nb\r\nc']);
add('anchors-no-m', 'line', '^b$', '', ['a\nb\nc']);
add('capture-unset-empty', 'capture', '(a)?()b\\1', 'd', ['b','ab']);
add('capture-alternation', 'capture', '(a)|(b)', 'dg', ['ab','ab','ab']);
add('capture-named', 'capture', '(?<first>a)?(?<empty>)b\\k<first>', 'd', ['b','aba']);
add('backref-forward', 'capture', '\\1(a)', '', ['a']);
add('backref-insensitive', 'capture', '(s)\\1', 'iu', ['sſ']);
add('lookbehind-positive', 'lookaround', '(?<=(a+))b', 'd', ['aab']);
add('lookbehind-negative', 'lookaround', '(?<!a)b', '', ['ab','cb']);
add('lookahead-unset', 'lookaround', '(?=(a))a|(b)', 'd', ['b','a']);
add('lookbehind-backref', 'lookaround', '(?<=(a)\\1)b', '', ['aab','ab']);
add('v-intersection', 'unicode-set', '[\\p{ASCII}&&\\p{Letter}]+', 'v', ['éabcΩ']);
add('v-subtraction', 'unicode-set', '[a-z--[aeiou]]+', 'v', ['aeiobcdf']);
add('v-subtraction-valid', 'unicode-set', '[[a-z]--[aeiou]]+', 'v', ['aeiobcdf']);
add('v-string-set', 'unicode-set', '[\\q{ab|cd|😀}]', 'gv', ['xabcd😀','xabcd😀','xabcd😀','xabcd😀']);
add('v-property-string', 'unicode-set', '\\p{RGI_Emoji_Flag_Sequence}', 'v', ['x🇹🇼y']);
add('u-negated-fold', 'unicode-set', '\\P{Lowercase_Letter}', 'iu', ['a','A']);
add('v-negated-fold', 'unicode-set', '\\P{Lowercase_Letter}', 'iv', ['a','A']);
add('v-q-empty-character-priority', 'unicode-set', '[\\q{|a}]', 'v', ['a','b']);
add('v-q-empty-mixed-priority', 'unicode-set', '[\\q{|ab}c]', 'v', ['ab','c','x']);
add('v-q-empty-backtrack', 'unicode-set', '[\\q{|a}]a', 'v', ['a','aa']);
add('v-single-ampersand-union', 'unicode-set', '[a&b]', 'v', ['a','&','b','c']);
add('u-negated-word-fold', 'unicode-set', '[\\W]', 'iu', ['ſ','K','a','!']);
add('inline-enable-i', 'modifier', '(?i:a)b', '', ['Ab','AB']);
add('inline-disable-i', 'modifier', '(?-i:a)b', 'i', ['aB','AB']);
add('inline-ms', 'modifier', '(?ms:^a.b$)', '', ['x\na\nb\ny']);
add('inline-invalid-g', 'invalid', '(?g:a)', '');

for (const flags of ['','g','y','gy','gu','yu','gyu','gv','yv']) {
  add(`state-astral-${flags || 'none'}`, 'state', '.', flags,
    [0,1,2,3,4,-1,1.75].map(setLastIndex => ({inputUnits:'😀x',setLastIndex}))
      .concat([{inputUnits:'😀x'},{inputUnits:'😀x'}]));
  add(`state-empty-${flags || 'none'}`, 'empty', '(?:)', flags,
    ['😀x','😀x',{inputUnits:'😀x',setLastIndex:1},'😀x',{inputUnits:'😀x',setLastIndex:3},'😀x']);
}
add('state-sticky-failure', 'state', 'b', 'y', ['ab',{inputUnits:'ab',setLastIndex:1},'ab']);
add('state-global-search', 'state', 'b', 'g', ['ab','ab','ab']);
for (const [id,input] of [['high',[0xd800]],['low',[0xdc00]],['pair',[0xd83d,0xde00]],
  ['mixed',[0xd800,0x61,0xdc00,0xd83d,0xde00]]]) {
  for (const flags of ['g','gu','gv']) add(`surrogate-dot-${id}-${flags}`, 'surrogate', '.', flags,
    Array.from({length:input.length+1}, () => input));
  add(`surrogate-literal-${id}`, 'surrogate', input, 'u', [input]);
}
add('surrogate-escaped-high', 'surrogate', '\\uD800', 'u', [[0xd800],[0xd800,0xdc00]]);
add('surrogate-class', 'surrogate', '[\\uD800-\\uDBFF]', 'g', [[0xd800,0xd83d,0xde00],[0xd800,0xd83d,0xde00]]);

if (new Set(cases.map(c => c.id)).size !== cases.length) throw new Error('duplicate case ID');
const counts = Object.fromEntries([...new Set(cases.map(c => c.family))].map(f => [f,cases.filter(c => c.family === f).length]));
const result = {schemaVersion:1, runtime:manifest.runtime, runnerSha256:manifest.runnerSha256, counts, cases};
fs.writeFileSync(path.join(path.dirname(manifestPath), 'oracle.json'), `${JSON.stringify(result,null,2)}\n`);
console.log(JSON.stringify({cases:cases.length, counts}));
