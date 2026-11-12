function gcd(a, b) {
    while (b != 0) {
        let t = b;
        b = a % b;
        a = t;
    }
    return a;
}

function lcm(a, b) {
    return a / gcd(a, b) * b;
}

console.log(gcd(48, 18));
console.log(lcm(4, 6));
console.log(gcd(17, 5));
