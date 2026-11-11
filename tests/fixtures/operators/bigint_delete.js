let log = window.console.log;
let String = window.String;

// BigInt literals
let big = 12345678901234567890n;
log(typeof big, String(big * 2n), String(0n), String(0x1fn), 10n > 9, 10n == 10, 10n === 10n);
log(String(-5n / 2n), String(2n ** 64n));

// `delete` of something that isn't a property: evaluated, always true
let calls = 0;
let f = () => calls++;
log(delete 1, delete f(), calls);

// update and compound operators keep BigInts as BigInts
let b = 1n;
b++;
++b;
b += 10n;
b **= 2n;
let old = b--;
log(String(old), String(b), typeof old);
let n = "5";
let oldN = n++;
log(oldN, typeof oldN, n);
