// for / while / break / continue and compound assignment.
let sum = 0;
for (let i = 0; i < 10; i++) {
    if (i % 2 == 0) {
        continue;
    }
    if (i > 7) {
        break;
    }
    sum += i;
}
console.log(sum);

let count = 0;
for (let i = 0; i < 3; i++) {
    for (let j = 0; j < 3; j++) {
        if (j == 2) {
            break;
        }
        count += 1;
    }
}
console.log(count);

let n = 27;
let steps = 0;
while (n != 1) {
    if (n % 2 == 0) {
        n /= 2;
    } else {
        n = 3 * n + 1;
    }
    steps++;
}
console.log(steps);
