// Test: Nullish coalescing and optional chaining
let x = null ?? "default";
console.log(x);  // default

let obj = { a: { b: 42 } };
console.log(obj?.a?.b);  // 42
console.log(obj?.c?.d);  // undefined

let val = 0 ?? "fallback";
console.log(val);  // 0 (not "fallback")

