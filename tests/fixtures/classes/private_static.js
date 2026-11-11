let log = window.console.log;
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};

class Registry {
    static #count = 0;
    static #items = [];
    static #prefix = "item-";
    static #make(name) {
        return Registry.#prefix + name;
    }
    static get #size() {
        return this.#items.length;
    }
    static set #size(v) {
        this.#items.length = v;
    }
    static add(name) {
        this.#count++;
        this.#items.push(Registry.#make(name));
        return this.#size;
    }
    static clear() {
        this.#size = 0;
        return this.#count;
    }
    static list() {
        return this.#items.join(",");
    }
    static {
        Registry.#items.push(this.#make("init"));
    }
}
log(Registry.add("a"), Registry.add("b"), Registry.list(), Registry.clear(), Registry.list());
log("#count" in Registry, Registry["#count"], window.Reflect.ownKeys(Registry).indexOf("#count"));

// static privates belong to the class itself, not to subclasses
class Sub extends Registry {}
log(errName(() => Sub.add("x")), errName(() => Sub.clear()));

// static initialisers see earlier static privates
class Seq {
    static #a = 1;
    static #b = Seq.#a + 1;
    static c = this.#b * 10;
    static #d = () => this.#b;
    static d() { return Seq.#d(); }
}
log(Seq.c, Seq.d());

// instance code can reach static privates through the class
class Both {
    static #instances = 0;
    #id;
    constructor() {
        this.#id = ++Both.#instances;
    }
    get id() { return this.#id; }
    static get total() { return Both.#instances; }
}
new Both();
log(new Both().id, Both.total);

// a static private method's `this` comes from how it's called
class S {
    static #who() { return this === S; }
    static check() { return [this.#who(), S.#who()].join(","); }
}
log(S.check());

// static private methods have a name and stay usable once escaped
class E {
    static #run() { return "ran"; }
    static get() { return this.#run; }
}
log(E.get().name, E.get()());
