let meta = #{ name: "stubs-throw-codes", description: "timestamp, sleep and exit throw a catchable capability code" };
let codes = [];
try { timestamp(); } catch (e) { codes.push(e.split(":")[0]); }
try { sleep(1); } catch (e) { codes.push(e.split(":")[0]); }
try { exit(1); } catch (e) { codes.push(e.split(":")[0]); }
complete(#{ codes: codes });
