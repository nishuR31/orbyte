// ============================================================
// 1. PRIMITIVE TYPES
// ============================================================

let name: string = "Nishu";
let age: number = 21;
let active: boolean = true;

let nothing: null = null;
let undefinedValue: undefined = undefined;

let anything: any = "anything";
let unknownValue: unknown = "unknown";

let numbers: number[] = [1, 2, 3];
let strings: Array<string> = ["a", "b", "c"];

let tuple: [string, number] = ["Nishu", 21];

type ID = string | number;
type Status = "pending" | "success" | "failed";

let id: ID = 100;
let status: Status = "success";

enum Direction {
    North,
    South,
    East,
    West
}

let direction: Direction = Direction.North;

// ============================================================
// 2. OBJECT TYPES
// ============================================================

type User = {
    id: number;
    name: string;
    age?: number;
    readonly createdAt: Date;
};

const user: User = {
    id: 1,
    name: "Nishu",
    createdAt: new Date()
};

interface Admin extends User {
    permissions: string[];
}

const admin: Admin = {
    id: 2,
    name: "Admin",
    createdAt: new Date(),
    permissions: ["read", "write"]
};

// ============================================================
// 3. FUNCTIONS
// ============================================================

function add(a: number, b: number): number {
    return a + b;
}

function greet(name: string = "Guest"): string {
    return `Hello ${name}`;
}

function multiply(a: number, b?: number): number {
    return a * (b ?? 1);
}

function sum(...values: number[]): number {
    return values.reduce((a, b) => a + b, 0);
}

const subtract = (a: number, b: number): number => a - b;

type Callback = (value: number) => string;

const callback: Callback = value => String(value);

console.log(add(10, 20));
console.log(greet());
console.log(multiply(10));
console.log(sum(1, 2, 3));

// ============================================================
// 4. FUNCTION OVERLOADING
// ============================================================

function process(value: string): string;
function process(value: number): number;

function process(value: string | number): string | number {
    return value;
}

console.log(process("Hello"));
console.log(process(100));

// ============================================================
// 5. GENERICS
// ============================================================

function identity<T>(value: T): T {
    return value;
}

function pair<T, U>(first: T, second: U): [T, U] {
    return [first, second];
}

function first<T>(items: T[]): T | undefined {
    return items[0];
}

function merge<T extends object, U extends object>(
    first: T,
    second: U
): T & U {
    return { ...first, ...second };
}

console.log(identity<number>(100));
console.log(identity<string>("Hello"));

console.log(pair<string, number>("Age", 21));

// ============================================================
// 6. GENERIC INTERFACES AND CLASSES
// ============================================================

interface Container<T> {
    value: T;
    getValue(): T;
}

class Box<T> implements Container<T> {
    constructor(public value: T) {}

    getValue(): T {
        return this.value;
    }
}

const box = new Box<number>(100);

console.log(box.getValue());

// ============================================================
// 7. CLASSES AND OOP
// ============================================================

abstract class Shape {
    abstract area(): number;

    describe(): string {
        return "Shape";
    }
}

class Circle extends Shape {
    constructor(public radius: number) {
        super();
    }

    override area(): number {
        return Math.PI * this.radius ** 2;
    }
}

class Rectangle extends Shape {
    constructor(
        private width: number,
        protected height: number
    ) {
        super();
    }

    area(): number {
        return this.width * this.height;
    }
}

const circle = new Circle(10);
const rectangle = new Rectangle(10, 20);

console.log(circle.area());
console.log(rectangle.area());

// ============================================================
// 8. PRIVATE AND STATIC MEMBERS
// ============================================================

class Account {
    static count = 0;

    #balance = 0;

    constructor(
        public readonly id: number
    ) {
        Account.count++;
    }

    get balance(): number {
        return this.#balance;
    }

    set balance(value: number) {
        this.#balance = value;
    }

    deposit(amount: number): void {
        this.#balance += amount;
    }

    static getCount(): number {
        return Account.count;
    }
}

const account = new Account(1);

account.deposit(100);

console.log(account.balance);
console.log(Account.getCount());

// ============================================================
// 9. UNION AND INTERSECTION TYPES
// ============================================================

type StringOrNumber = string | number;

type Person = {
    name: string;
};

type Employee = {
    salary: number;
};

type Worker = Person & Employee;

const worker: Worker = {
    name: "Nishu",
    salary: 50000
};

// ============================================================
// 10. TYPE NARROWING
// ============================================================

function processValue(value: string | number): string {
    if (typeof value === "string") {
        return value.toUpperCase();
    }

    return value.toFixed(2);
}

interface Cat {
    meow(): void;
}

interface Dog {
    bark(): void;
}

function isCat(animal: Cat | Dog): animal is Cat {
    return "meow" in animal;
}

function handleAnimal(animal: Cat | Dog): void {
    if (isCat(animal)) {
        animal.meow();
    } else {
        animal.bark();
    }
}

// ============================================================
// 11. DISCRIMINATED UNIONS
// ============================================================

type Result =
    | { success: true; data: string }
    | { success: false; error: string };

function handleResult(result: Result): string {
    if (result.success) {
        return result.data;
    }

    return result.error;
}

// ============================================================
// 12. TYPE ASSERTIONS
// ============================================================

const input = document.querySelector("input") as HTMLInputElement;

const value = input?.value;

const unknownData: unknown = "Hello";

const stringData = unknownData as string;

console.log(stringData);

// ============================================================
// 13. UTILITY TYPES
// ============================================================

type PartialUser = Partial<User>;
type RequiredUser = Required<User>;
type ReadonlyUser = Readonly<User>;

type PickUser = Pick<User, "id" | "name">;
type OmitUser = Omit<User, "age">;

type RecordUser = Record<string, User>;

type Excluded = Exclude<string | number, string>;
type Extracted = Extract<string | number, string>;

type SafeString = NonNullable<string | null | undefined>;

type UserKeys = keyof User;
type UserName = User["name"];

// ============================================================
// 14. CONDITIONAL TYPES
// ============================================================

type IsString<T> = T extends string ? true : false;

type ElementType<T> =
    T extends (infer U)[] ? U : T;

type PromiseValue<T> =
    T extends Promise<infer U> ? U : T;

type FunctionReturn<T> =
    T extends (...args: any[]) => infer R ? R : never;

type Example1 = IsString<string>;
type Example2 = ElementType<number[]>;
type Example3 = PromiseValue<Promise<string>>;

// ============================================================
// 15. MAPPED TYPES
// ============================================================

type Optional<T> = {
    [K in keyof T]?: T[K];
};

type Mutable<T> = {
    -readonly [K in keyof T]: T[K];
};

type RequiredProperties<T> = {
    [K in keyof T]-?: T[K];
};

type StringProperties<T> = {
    [K in keyof T]: string;
};

// ============================================================
// 16. TEMPLATE LITERAL TYPES
// ============================================================

type EventName = "click" | "submit";

type EventHandler = `${EventName}:handler`;

type Route = `/api/${string}`;

const route: Route = "/api/users";

const eventName: EventHandler = "click:handler";

// ============================================================
// 17. INDEX SIGNATURES
// ============================================================

interface Dictionary {
    [key: string]: number;
}

const dictionary: Dictionary = {
    one: 1,
    two: 2
};

// ============================================================
// 18. ASYNC TYPES
// ============================================================

async function fetchUser(): Promise<User> {
    return {
        id: 1,
        name: "Nishu",
        createdAt: new Date()
    };
}

async function run(): Promise<void> {
    const result = await fetchUser();
    console.log(result);
}

run();

// ============================================================
// 19. ASSERTION FUNCTIONS
// ============================================================

function assertDefined<T>(
    value: T | null | undefined
): asserts value is T {
    if (value == null) {
        throw new Error("Value is undefined");
    }
}

// ============================================================
// 20. CONST ASSERTIONS
// ============================================================

const config = {
    environment: "production",
    version: 1
} as const;

// ============================================================
// 21. SATISFIES OPERATOR
// ============================================================

type Config = {
    port: number;
    host: string;
};

const serverConfig = {
    port: 3000,
    host: "localhost"
} satisfies Config;

// ============================================================
// 22. NAMESPACE
// ============================================================

namespace Mathematics {
    export const PI = 3.14159;

    export function square(value: number): number {
        return value * value;
    }
}

console.log(Mathematics.square(5));

// ============================================================
// 23. DECORATORS (NEW DECORATOR SYNTAX)
// ============================================================

function logged(
    target: Function,
    context: ClassMethodDecoratorContext
) {
    return function (this: unknown, ...args: unknown[]) {
        console.log(`Calling ${String(context.name)}`);
        return target.apply(this, args);
    };
}

class Service {
    @logged
    execute(): string {
        return "Executed";
    }
}

// ============================================================
// 24. JSX/TSX-TYPE EXPRESSIONS
// ============================================================

const jsxElement = <div>Hello World</div>;

const tsxElement = (
    <div className="container">
        <h1>{name}</h1>
        <p>{age}</p>
    </div>
);

// ============================================================
// 25. MODULE EXPORTS
// ============================================================

export type { User, Admin };

export const version = "1.0.0";

export function calculate(a: number, b: number): number {
    return a + b;
}

export default function main(): string {
    return "TypeScript compiler test";
}

// ============================================================
// 26. INTENTIONAL TYPE ERRORS
// ============================================================

// Uncomment individually to test type-checking failures:
//
// let invalidNumber: number = "hello";
// let invalidString: string = 100;
//
// function invalid(a: number): number {
//     return "wrong";
// }
//
// const invalidStatus: Status = "unknown";
//
// const invalidUser: User = {
//     id: "wrong",
//     name: "Nishu",
//     createdAt: new Date()
// };
