// Test: Spread and rest operators
function sum(...args) {
  let total = 0;
  for (let n of args) total += n;
  return total;
}
console.log(sum(1, 2, 3, 4, 5));  // 15

let a = [1, 2];
let b = [3, 4];
let c = [...a, ...b];
console.log(c.length);  // 4

let { x, ...rest } = { x: 1, y: 2, z: 3 };
console.log(x);           // 1
console.log(Object.keys(rest).length);  // 2

