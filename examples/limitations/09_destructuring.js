// Test: Destructuring assignment
let [a, b, c] = [1, 2, 3];
console.log(a + b + c);  // 6

let { x, y } = { x: 10, y: 20 };
console.log(x + y);  // 30

function coords({ x, y }) {
  return x * y;
}
console.log(coords({ x: 3, y: 4 }));  // 12

