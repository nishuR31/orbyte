// IEEE-754 corner cases that the optimiser and the JIT must agree on.
console.log(0 / 0);
console.log(1 / 0);
console.log(-1 / 0);
console.log(0 / 0 == 0 / 0);
console.log(0 / 0 != 0 / 0);
console.log(-7 % 3);
console.log(7 % -3);
console.log(5 % 0);
console.log(0.1 + 0.2 == 0.3);
console.log(1 / 4);
console.log(-0 == 0);
