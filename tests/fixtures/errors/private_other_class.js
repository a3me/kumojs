// compile-error: undeclared private name `#x`
class A {
    #x = 1;
}
class B {
    m(o) {
        return #x in o;
    }
}
