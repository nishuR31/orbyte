function isPrime(n) {
    if (n < 2) {
        return false;
    }
    for (let d = 2; d * d <= n; d++) {
        if (n % d == 0) {
            return false;
        }
    }
    return true;
}

let count = 0;
let last = 0;
for (let n = 0; n < 1000; n++) {
    if (isPrime(n)) {
        count++;
        last = n;
    }
}
console.log(count);
console.log(last);
