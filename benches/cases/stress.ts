function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}
let acc = 0;
for (let i = 0; i < 8; i++) acc += fib(24);
console.log(acc);
