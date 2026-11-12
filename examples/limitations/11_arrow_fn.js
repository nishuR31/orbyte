// Test: Arrow functions
const add = (a, b) => a + b;
console.log(add(3, 4));  // 7

const square = x => x * x;
console.log(square(5));  // 25

const nums = [1, 2, 3, 4, 5];
let evens = nums.filter(n => n % 2 === 0);
console.log(evens.length);  // 2

