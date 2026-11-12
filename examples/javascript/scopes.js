function pick(c) {
  if (c > 0) {
    var r = 100;
    return r + 1;
  }
  return 0;
}
let x = 1;
{
  let z = 5;
  let x = z * 10;
  console.log(x);
}
console.log(x);
console.log(pick(1));
console.log(pick(0));
