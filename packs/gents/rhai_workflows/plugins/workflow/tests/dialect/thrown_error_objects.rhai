let meta = #{ name: "thrown-error-objects", description: "values thrown by the script arrive in catch as they were thrown" };
fn risky(n) { if n > 2 { throw #{ n: n }; } n }
let log = [];
try { throw #{ code: 7, reason: "boom" }; } catch (e) { log.push(e.code); log.push(e.reason); }
try { throw "plain"; } catch (e) { log.push(e); }
try { try { throw 1; } catch (e) { throw e + 1; } } catch (e) { log.push(e); }
let seen = [];
for i in 0..5 { try { seen.push(risky(i)); } catch (e) { seen.push(-e.n); } }
complete(#{ log: log, seen: seen });
