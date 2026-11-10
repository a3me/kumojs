let obj = { a: 1, "b": 2, 3: "three" };
obj.c = obj.a + obj.b;
obj["d"] = [obj.c, obj[3]];
window.console.log(obj.a, obj.b, obj.c, obj.d[0], obj.d[1], obj[3]);
window.console.log(delete obj.a, obj.a, "a" in obj);
let arr = [1, , 3];
window.console.log(arr.length, arr[1], arr[2]);
window.console.log(window.Math.max(1, 5, 3), window.String(12));
