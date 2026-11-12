// ============================================================
// 1. VARIABLES AND DATA TYPES
// ============================================================

var oldVar = 10;
let age = 21;
const name = "Nishu";

const number = 100;
const float = 3.14159;
const boolean = true;
const nothing = null;
let undefinedValue;
const big = 12345678901234567890n;
const symbol = Symbol("id");

// ============================================================
// 2. OPERATORS
// ============================================================

let x = 10;
let y = 3;

console.log(x + y, x - y, x * y, x / y, x % y, x ** y);

console.log(x == y, x === y, x != y, x !== y);
console.log(x > y, x < y, x >= y, x <= y);

console.log(true && false, true || false, !true);
console.log(null ?? "default");
console.log(x > y ? "yes" : "no");

x += 5;
x -= 2;
x *= 3;
x /= 2;
x %= 4;
x **= 2;

x++;
x--;
++x;
--x;

console.log(5 & 3, 5 | 3, 5 ^ 3, ~5);
console.log(5 << 2, 20 >> 2, 20 >>> 2);

// ============================================================
// 3. STRINGS
// ============================================================

const text = "JavaScript";
const template = `Hello ${name}, age ${age}`;

console.log(text.length);
console.log(text.toUpperCase());
console.log(text.toLowerCase());
console.log(text.includes("Script"));
console.log(text.startsWith("Java"));
console.log(text.endsWith("Script"));
console.log(text.substring(0, 4));
console.log(text.slice(0, 4));
console.log(text.replace("Java", "Type"));
console.log(text.split(""));
console.log("  hello  ".trim());
console.log("hello".padStart(10, "*"));

const multiline = `First line
Second line`;

console.log(multiline);

// ============================================================
// 4. ARRAYS
// ============================================================

const numbers = [1, 2, 3, 4, 5];

numbers.push(6);
numbers.pop();
numbers.unshift(0);
numbers.shift();

console.log(numbers.map(n => n * 2));
console.log(numbers.filter(n => n > 2));
console.log(numbers.reduce((a, b) => a + b, 0));
console.log(numbers.find(n => n === 3));
console.log(numbers.findIndex(n => n === 3));
console.log(numbers.some(n => n > 3));
console.log(numbers.every(n => n > 0));
console.log(numbers.includes(3));
console.log(numbers.slice(1, 3));
console.log(numbers.concat([6, 7]));
console.log(numbers.join(","));
console.log(numbers.flat());

const nested = [[1, 2], [3, 4]];
console.log(nested.flat());

const sparse = [1, , 3];

// ============================================================
// 5. OBJECTS
// ============================================================

const user = {
    name: "Nishu",
    age: 21,
    active: true,
    address: {
        city: "Ranchi",
        state: "Jharkhand"
    },

    greet() {
        return `Hello ${this.name}`;
    }
};

console.log(user.name);
console.log(user["age"]);
console.log(user.greet());

user.age = 22;
user.email = "test@example.com";

console.log(Object.keys(user));
console.log(Object.values(user));
console.log(Object.entries(user));
console.log(Object.hasOwn(user, "name"));

const { name: username, age: userAge } = user;
const { address: { city } } = user;

console.log(username, userAge, city);

const copiedUser = { ...user };
const merged = { ...user, role: "developer" };

console.log(merged);

// ============================================================
// 6. CONTROL FLOW
// ============================================================

if (age > 18) {
    console.log("Adult");
} else if (age === 18) {
    console.log("Exactly 18");
} else {
    console.log("Minor");
}

switch (age) {
    case 18:
        console.log("18");
        break;

    case 21:
        console.log("21");
        break;

    default:
        console.log("Other");
}

for (let i = 0; i < 5; i++) {
    console.log(i);
}

let count = 0;

while (count < 3) {
    count++;
}

do {
    count--;
} while (count > 0);

for (const value of numbers) {
    console.log(value);
}

for (const key in user) {
    console.log(key);
}

outer:
for (let i = 0; i < 5; i++) {
    for (let j = 0; j < 5; j++) {
        if (j === 2) continue;
        if (i === 3) break outer;
    }
}

// ============================================================
// 7. FUNCTIONS
// ============================================================

function add(a, b) {
    return a + b;
}

const subtract = function (a, b) {
    return a - b;
};

const multiply = (a, b) => a * b;

const square = x => x * x;

const greet = (name = "Guest") => `Hello ${name}`;

function sum(...values) {
    return values.reduce((a, b) => a + b, 0);
}

console.log(add(10, 20));
console.log(subtract(20, 10));
console.log(multiply(5, 5));
console.log(square(5));
console.log(greet());
console.log(sum(1, 2, 3, 4));

// Recursion
function factorial(n) {
    if (n <= 1) return 1;
    return n * factorial(n - 1);
}

console.log(factorial(5));

// Closures
function createCounter() {
    let counter = 0;

    return function () {
        return ++counter;
    };
}

const counter = createCounter();

console.log(counter(), counter(), counter());

// IIFE
(function () {
    console.log("Immediately invoked");
})();

// Higher-order functions
function execute(callback) {
    return callback(100);
}

console.log(execute(x => x * 2));

// ============================================================
// 8. CLASSES AND INHERITANCE
// ============================================================

class Animal {
    constructor(name) {
        this.name = name;
    }

    speak() {
        return `${this.name} makes a sound`;
    }

    static category() {
        return "Animal";
    }
}

class Dog extends Animal {
    constructor(name, breed) {
        super(name);
        this.breed = breed;
    }

    speak() {
        return `${this.name} barks`;
    }
}

const dog = new Dog("Rocky", "Labrador");

console.log(dog.speak());
console.log(Animal.category());

// Getters and setters
class Account {
    #balance = 0;

    get balance() {
        return this.#balance;
    }

    set balance(value) {
        this.#balance = value;
    }

    deposit(amount) {
        this.#balance += amount;
    }
}

const account = new Account();
account.deposit(100);
console.log(account.balance);

// Static fields
class Config {
    static version = "1.0";
}

console.log(Config.version);

// ============================================================
// 9. PROTOTYPES
// ============================================================

const prototype = {
    greet() {
        return "Hello from prototype";
    }
};

const obj = Object.create(prototype);

console.log(obj.greet());
console.log(Object.getPrototypeOf(obj));

Object.defineProperty(obj, "id", {
    value: 100,
    writable: false,
    enumerable: true
});

console.log(Object.keys(obj));

// ============================================================
// 10. MAP, SET, WEAKMAP, WEAKSET
// ============================================================

const map = new Map();

map.set("name", "Nishu");
map.set("age", 21);

console.log(map.get("name"));
console.log(map.has("age"));
console.log(map.size);

const set = new Set([1, 2, 2, 3]);

console.log(set.size);
console.log([...set]);

const weakMap = new WeakMap();
const weakSet = new WeakSet();

const objectKey = {};

weakMap.set(objectKey, "value");
weakSet.add(objectKey);

console.log(weakMap.get(objectKey));

// ============================================================
// 11. EXCEPTIONS
// ============================================================

try {
    throw new Error("Test error");
} catch (error) {
    console.log(error.message);
} finally {
    console.log("Finally executed");
}

class CustomError extends Error {
    constructor(message) {
        super(message);
        this.name = "CustomError";
    }
}

try {
    throw new CustomError("Custom exception");
} catch (error) {
    console.log(error.name);
}

// ============================================================
// 12. PROMISES AND ASYNC/AWAIT
// ============================================================

const promise = new Promise((resolve, reject) => {
    resolve(100);
});

promise
    .then(value => value * 2)
    .then(console.log)
    .catch(console.error)
    .finally(() => console.log("Completed"));

async function fetchData() {
    const result = await promise;
    return result;
}

async function runAsync() {
    console.log(await fetchData());

    const results = await Promise.all([
        Promise.resolve(1),
        Promise.resolve(2),
        Promise.resolve(3)
    ]);

    console.log(results);

    console.log(await Promise.allSettled([
        Promise.resolve("OK"),
        Promise.reject("FAIL")
    ]));

    console.log(await Promise.race([
        Promise.resolve("FIRST"),
        Promise.resolve("SECOND")
    ]));
}

runAsync();

// ============================================================
// 13. GENERATORS AND ITERATORS
// ============================================================

function* generator() {
    yield 1;
    yield 2;
    yield 3;
}

const iterator = generator();

console.log(iterator.next());
console.log(iterator.next());
console.log(iterator.next());

const iterable = {
    values: [10, 20, 30],

    [Symbol.iterator]() {
        let index = 0;
        const values = this.values;

        return {
            next() {
                if (index < values.length) {
                    return {
                        value: values[index++],
                        done: false
                    };
                }

                return { done: true };
            }
        };
    }
};

for (const value of iterable) {
    console.log(value);
}

// ============================================================
// 14. OPTIONAL CHAINING AND NULLISH COALESCING
// ============================================================

const data = {};

console.log(data?.user?.name);
console.log(data?.user?.name ?? "Unknown");

// ============================================================
// 15. REGEX
// ============================================================

const regex = /^[a-z]+$/i;

console.log(regex.test("Hello"));
console.log("hello123".match(/\d+/));

// ============================================================
// 16. PROXY AND REFLECT
// ============================================================

const target = { name: "Nishu" };

const proxy = new Proxy(target, {
    get(target, property) {
        return Reflect.get(target, property);
    },

    set(target, property, value) {
        return Reflect.set(target, property, value);
    }
});

proxy.name = "Developer";

console.log(proxy.name);
console.log(Reflect.ownKeys(target));

// ============================================================
// 17. JSON AND DATE
// ============================================================

const json = JSON.stringify(user);
const parsed = JSON.parse(json);

console.log(parsed.name);

const date = new Date();

console.log(date.getFullYear());
console.log(date.getMonth());
console.log(date.toISOString());

// ============================================================
// 18. DYNAMIC JAVASCRIPT BEHAVIOR
// ============================================================

function dynamic(value) {
    return value + value;
}

console.log(dynamic(10));
console.log(dynamic("Hello"));
console.log(dynamic([1, 2]));
console.log(dynamic(null));

// Type coercion
console.log("5" + 5);
console.log("5" - 2);
console.log([] + []);
console.log([] + {});
console.log(null == undefined);
console.log(null === undefined);

// NaN and signed zero
console.log(NaN === NaN);
console.log(Object.is(NaN, NaN));
console.log(0 === -0);
console.log(Object.is(0, -0));

// Hoisting
console.log(hoisted());

function hoisted() {
    return "Hoisted function";
}

// typeof undeclared identifier
console.log(typeof unknownVariable);

// ============================================================
// 19. SYMBOLS AND BIGINT
// ============================================================

const unique = Symbol("unique");

console.log(unique);

const bigNumber = 100000000000000000000n;

console.log(bigNumber + 10n);

// ============================================================
// 20. MODULE-RELATED SYNTAX
// ============================================================

// Test separately as an ES module:
// import/export require module-aware compilation.

export const moduleValue = 100;
export function moduleFunction() {
    return "Module works";
}
export default moduleValue;
