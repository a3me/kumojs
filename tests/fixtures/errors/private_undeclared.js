// compile-error: undeclared private name `#missing`
class A {
    m() {
        return this.#missing;
    }
}
