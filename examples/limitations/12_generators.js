// Test: Generators and iterators
function* range(start, end) {
  for (let i = start; i < end; i++) {
    yield i;
  }
}
let total = 0;
for (let v of range(1, 6)) {
  total += v;
}
console.log(total);  // 15

