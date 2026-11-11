// classes: fields, private members, accessors, statics, super
class Base {
    #secret = 1;
    count = 0;
    static made = 0;
    get secret() {
        return this.#secret;
    }
    #bump() {
        return ++this.count;
    }
    run() {
        return this.#bump() + (#secret in this ? 1 : 0);
    }
}
class Sub extends Base {
    extra = "x";
    constructor() {
        super();
        Sub.made++;
    }
    run() {
        return super.run() * 2;
    }
}
new Sub().run();
