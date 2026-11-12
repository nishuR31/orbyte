// Test: Regular expressions
let re = /hello/i;
console.log(re.test("Hello World"));  // true
let matches = "foo bar baz".match(/\b\w+/g);
console.log(matches.length);  // 3

