let meta = #{ name: "recursion", description: "recursive fns within the call-depth limit" };
fn fact(n) { if n <= 1 { 1 } else { n * fact(n - 1) } }
fn fib(n) { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }
fn gcd(a, b) { if b == 0 { a } else { gcd(b, a % b) } }
complete(#{ fact: fact(5), fib: fib(5), gcd: gcd(48, 18) });
