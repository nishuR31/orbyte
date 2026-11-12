// Test: Type coercion (intentionally JS-loose)
console.log(1 + "2");     // "12" (number + string -> string)
console.log("5" - 2);     // 3   (string - number -> number)
console.log(true + 1);    // 2   (bool + number -> number)
console.log(null + 1);    // 1
console.log("" == false); // true

