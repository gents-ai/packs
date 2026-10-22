let meta = #{ name: "integer-overflow-to-float", description: "a literal past i64 is a float, integer arithmetic past it throws" };
let top = 9223372036854775807;
let past = 9223372036854775808;
let caught = false;
try { let sum = top + 1; } catch (e) { caught = true; }
complete(#{
    top: top,
    past_type: type_of(past),
    top_type: type_of(top),
    caught: caught,
    widened: top.to_float() + 1.0,
    half: 7 / 2,
    fhalf: 7.0 / 2.0,
});
