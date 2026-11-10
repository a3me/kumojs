let log = window.console.log;
let obj = { b: 1, a: 2, 10: "ten", 1.5: "x", "c d": 3, a: "dup" };
log(window.Object.keys(obj).join(","), obj.a, obj[10], obj["1.5"]);
let holes = [1, , 3, , ];
log(holes.length, 1 in holes, 0 in holes, holes);
let nested = { list: [{ v: 1 }, { v: 2 }], fn: function () { return this.list.length; } };
log(nested.list[1].v, nested.fn());
