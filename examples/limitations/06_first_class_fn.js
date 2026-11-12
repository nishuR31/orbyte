// Test: First-class functions (pass as argument, return as value)
function apply(fn, x) {
  return fn(x);
}
function double(n) {
  return n * 2;
}
console.log(apply(double, 5));  // 10

let ops = [double, function(n) { return n + 1; }];
console.log(ops[0](3));  // 6
console.log(ops[1](3));  // 4

