let log = window.console.log;

let name = "kumo", n = 3;
log(`plain`, `hi ${name}!`, `${n} + ${n} = ${n + n}`, `${"a"}${"b"}`, `edge ${""}`);
log(`multi
line`, `esc \tA\x42 \` \${not}`, `nested ${`inner ${n}`}`);

// values convert with ToString (toString), not like `+` (valueOf)
let both = { toString: () => "str", valueOf: () => "val" };
log(`${both}`, "" + both, `${null} ${undefined} ${[1, 2]} ${{}}`);
try { `${window.Symbol("s")}`; } catch (e) { log(e instanceof window.TypeError); }

// tagged templates
function tag(strings, ...values) {
    return strings.join("|") + " / " + values.join(",") + " / " + strings.raw.join("|");
}
log(tag`a${1}b${2}c`, tag``, tag`\n${"x"}`);
log(window.String.raw`A ${1 + 1} \n`);

// the strings array is frozen and the same object each time the site runs
let seen = [];
let keep = s => { seen.push(s); return s; };
for (let i = 0; i < 2; i++) keep`site`;
keep`site`;
log(seen[0] === seen[1], seen[0] === seen[2], window.Object.isFrozen(seen[0]), window.Object.isFrozen(seen[0].raw));

// a member tag gets its object as `this`
let obj = { prefix: ">", t(strings, v) { return this.prefix + strings[0] + v; } };
log(obj.t`x${1}`);

// invalid escapes are allowed in tagged templates (cooked is undefined)
let cooked = s => s[0] === undefined ? "undefined:" + s.raw[0] : s[0];
log(cooked`\unicode`);
