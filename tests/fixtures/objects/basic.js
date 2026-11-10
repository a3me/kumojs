let obj = { a: 1, "b": 2, 3: "three" };
obj.c = obj.a + obj.b;
obj["d"] = [obj.c, obj[3]];
console.log(obj.a, obj.b, obj.c, obj.d[0], obj.d[1], obj[3]);
console.log(delete obj.a, obj.a, "a" in obj);
let arr = [1, , 3];
console.log(arr.length, arr[1], arr[2]);
console.log(Math.max(1, 5, 3), String(12));
