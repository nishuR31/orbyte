// Test: Closures and captured variables
function makeCounter() {
  let count = 0;
  return function() {
    count += 1;
    return count;
  };
}
let counter = makeCounter();
console.log(counter());
console.log(counter());
console.log(counter());

