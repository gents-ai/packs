let meta = #{ name: "for-and-while-loops", description: "for over ranges and arrays, while, break and continue" };
let sum = 0;
for i in 0..5 { sum += i; }
let skipped = 0;
for i in 0..10 {
    if i % 2 == 1 { continue; }
    if i > 6 { break; }
    skipped += i;
}
let n = 1;
while n < 100 { n *= 3; }
let countdown = [];
let k = 3;
while k > 0 { countdown.push(k); k -= 1; }
complete(#{ sum: sum, skipped: skipped, n: n, countdown: countdown });
