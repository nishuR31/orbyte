// Test: try/catch/throw
function divide(a, b) {
  if (b === 0) {
    throw new Error("division by zero");
  }
  return a / b;
}
try {
  console.log(divide(10, 2));
  console.log(divide(10, 0));
} catch (e) {
  console.log("caught: " + e.message);
}

