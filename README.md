# kumojs

kumojs is an javascript compiler written in Rust, and an accompanying virtual machine / interpreter written in javascript.

- Input is parsed in strict mode (as a module), so features like `with` are not supported.
- ES3 support is the current target baseline, with ES5+ syntax support to come.
- Built-ins (`Math`, `String`, `Date`, `Array`, etc.) are taken from the host global scope, not reimplemented (this may change).
- Priorities are obfuscation first and speed second.