let log = window.console.log;
let Promise = window.Promise;
let O = window.Object;
let show = (r) => window.JSON.stringify(r);

async function* agen() {
    log("agen start");
    let x = yield 1;
    log("agen got", x);
    yield Promise.resolve(2); // yielded promises are awaited
    try {
        yield 3;
    } finally {
        log("agen finally");
    }
    return "end";
}

async function main() {
    // next() returns promises; requests queue up and run in order
    let g = agen();
    log(O.prototype.toString.call(g), typeof g[window.Symbol.asyncIterator]);
    let ps = [g.next(), g.next("sent"), g.next(), g.next(), g.next()];
    log("queued");
    for (let p of ps) log(show(await p));

    // return() mid-way runs finally; throw() on a finished generator rejects
    let g2 = agen();
    await g2.next();
    await g2.next();
    await g2.next();
    log(show(await g2.return("early")));
    try {
        await g2.throw(new window.Error("after done"));
    } catch (e) {
        log("rejected", e.message);
    }
    log(show(await g2.return(Promise.resolve("awaited return"))));

    // for await over async generators, sync iterables of promises, and arrays
    let out = [];
    for await (let v of agen()) out.push(v);
    for await (let v of [Promise.resolve("p1"), "plain", Promise.resolve("p2")]) out.push(v);
    function* syncGen() {
        yield Promise.resolve("s1");
        yield "s2";
    }
    for await (const v of syncGen()) out.push(v);
    log(out.join(","));

    // break / return / throw out of for await close the iterator (awaiting return())
    for await (let v of agen()) {
        log("first", v);
        break;
    }
    let found = await (async () => {
        for await (let v of agen()) if (v === 2) return "found 2";
    })();
    log(found);
    try {
        for await (let v of agen()) throw new window.Error("thrown in loop");
    } catch (e) {
        log("caught", e.message);
    }

    // a hand-written async iterator
    let events = [];
    let custom = {
        [window.Symbol.asyncIterator]() {
            let i = 0;
            return {
                next() {
                    events.push("next");
                    return Promise.resolve({ value: i, done: i++ >= 2 });
                },
                return() {
                    events.push("return");
                    return Promise.resolve({ done: true });
                },
            };
        },
    };
    for await (let v of custom) events.push(v);
    for await (let v of custom) {
        events.push(v);
        break;
    }
    log(events.join(" "));

    // yield* in async generators delegates to async and sync iterables
    async function* delegating() {
        let r = yield* agen();
        log("delegate result", r);
        yield* ["x", "y"];
    }
    let d = [];
    for await (let v of delegating()) d.push(v);
    log(d.join(","));

    // an error thrown in an async generator rejects next() and finishes it
    async function* bad() {
        yield 1;
        throw new window.TypeError("bad gen");
    }
    let b = bad();
    await b.next();
    try {
        await b.next();
    } catch (e) {
        log("next rejected", e.name);
    }
    log(show(await b.next()));

    // await inside async generators, and the arguments of the call
    async function* withAwait(a, b) {
        let v = await Promise.resolve(a);
        yield v + b;
        yield await new Promise((r) => r("later"));
    }
    let wa = [];
    for await (let v of withAwait(1, 2)) wa.push(v);
    log(wa.join(","));

    // methods
    class Stream {
        constructor(items) {
            this.items = items;
        }
        async *[window.Symbol.asyncIterator]() {
            for (let item of this.items) yield item;
        }
    }
    let s = [];
    for await (let v of new Stream(["m1", "m2"])) s.push(v);
    log(s.join(","));
    return "main done";
}
main().then(log, (e) => log("main failed", e));
log("main started");
