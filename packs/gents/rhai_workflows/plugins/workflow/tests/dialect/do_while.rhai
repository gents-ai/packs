let meta = #{ name: "do-while", description: "do-while and do-until run the body once before testing" };
let n = 0;
do { n += 1; } while n < 5;
let m = 10;
do { m -= 3; } until m < 0;
let once = 0;
do { once += 1; } while false;
complete(#{ n: n, m: m, once: once });
