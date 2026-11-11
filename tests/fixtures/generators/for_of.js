let log = window.console.log;
let JSON = window.JSON;
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};

// for-of over generators: values, yield* pass-through, return values are ignored
function* letters() {
    yield "a";
    yield* ["b", "c"];
    yield* (function* () {
        yield "d";
        return "ignored";
    })();
    return "also ignored";
}
let out = [];
for (let v of letters()) out.push(v);
log(out.join(""));

// a throw inside the generator comes out of the loop; the generator is finished
function* failing() {
    yield 1;
    throw new window.TypeError("from gen");
}
let fg = failing();
let seen = [];
log(errName(() => {
    for (let v of fg) seen.push(v);
}), seen.join(","), JSON.stringify(fg.next()));

// break closes the generator (finally runs); a fresh loop over it gets nothing
function* closable() {
    try {
        yield 1;
        yield 2;
    } finally {
        log("closed");
    }
}
let cg = closable();
for (let v of cg) {
    log("first", v);
    break;
}
let after = [];
for (let v of cg) after.push(v);
log("after", after.length);

// continuing a partly used generator, and nested loops over one generator
function* nums() {
    for (let i = 0; i < 6; i++) yield i;
}
let ng = nums();
ng.next();
let pairs = [];
for (let a of ng) {
    for (let b of ng) {
        pairs.push(a + ":" + b);
        break;
    }
}
log(pairs.join(" "));

// a generator iterating itself is "already running"
let selfGen;
function* selfish() {
    for (let v of selfGen) yield v;
}
selfGen = selfish();
log(errName(() => selfGen.next()));

// generators that yield inside try/catch, with throws caught in the generator
function* resilient() {
    for (let i = 0; i < 3; i++) {
        try {
            if (i === 1) throw new window.Error("skip");
            yield i;
        } catch (e) {
            yield "caught " + e.message;
        }
    }
}
log([...resilient()].join(", "));
let r = [];
for (let v of resilient()) r.push(v);
log(r.join(", "));

// destructuring heads with generator values, and generators passed to host code
function* entries() {
    yield ["x", 1];
    yield ["y", 2];
}
let keys = [];
for (const [k, v] of entries()) keys.push(k + v);
log(keys.join(","), window.Object.fromEntries(entries()).y, new window.Map(entries()).size);
