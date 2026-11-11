let log = window.console.log;
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};

class Brand {
    #field;
    #method() {}
    get #acc() { return 1; }
    static #staticField = 1;
    static #staticMethod() {}
    static isBrand(o) {
        return #field in o;
    }
    static check(o) {
        return [#field in o, #method in o, #acc in o, #staticField in o, #staticMethod in o].join(",");
    }
}
log(Brand.check(new Brand()), Brand.check(Brand), Brand.check({}));
log(Brand.isBrand(new Brand()), Brand.isBrand(function () {}), Brand.isBrand([]));

// the right side must be an object
log(errName(() => Brand.isBrand(1)), errName(() => Brand.isBrand("s")), errName(() => Brand.isBrand(null)),
    errName(() => Brand.isBrand(undefined)));

// subclass instances pass, look-alikes and proxies don't
class Sub extends Brand {}
let sub = new Sub();
log(Brand.isBrand(sub), Brand.isBrand(window.Object.create(Brand.prototype)),
    Brand.isBrand(new window.Proxy(new Brand(), {})));

// `#x in o` works for a brand check before reading
class Safe {
    #v = "v";
    static read(o) {
        return #v in o ? o.#v : "none";
    }
}
log(Safe.read(new Safe()), Safe.read({}));

// usable in any expression position, including `!` and conditions
class Cond {
    #x = 1;
    static not(o) {
        return !(#x in o);
    }
    static count(list) {
        let n = 0;
        for (let i = 0; i < list.length; i++) if (#x in list[i]) n++;
        return n;
    }
}
log(Cond.not({}), Cond.not(new Cond()), Cond.count([new Cond(), {}, new Cond()]));
