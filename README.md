# kumojs
雲, meaning "cloud" to symbolize obscurity.

## what is it?

kumojs is an "es3" (see below missing features) javascript compiler written in Rust, and an accompanying virtual machine / interpreter written in javascript - it is intended to be used for obfuscating javascript code and interpreting it in a browser.

virtualized obfuscation is a growing trend in javascript obfuscation, and kumojs is an attempt to demonstrate an open source implementation of this technique.

## missing es3 features

- [ ] `with` statement
- [ ] `eval` statement
- [ ] `arguments` object
- [ ] `new.target` property