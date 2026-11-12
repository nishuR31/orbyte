// Short-circuit evaluation: the right operand must only run when needed.
function yes(x) {
    console.log(x);
    return true;
}

function no(x) {
    console.log(x);
    return false;
}

console.log(no(1) && yes(2));
console.log(yes(3) || no(4));
console.log(yes(5) && no(6));
console.log(no(7) || yes(8));

function isEven(n) {
    if (n == 0) {
        return true;
    }
    return isOdd(n - 1);
}

function isOdd(n) {
    if (n == 0) {
        return false;
    }
    return isEven(n - 1);
}

console.log(isEven(10));
console.log(isOdd(7));
console.log(isEven(3));

let a = 1;
let b = 2;
console.log((a > 0) == (b > 5 && a > 0));
console.log((a > 0) != (b > 5 || a > 5));
