import ms from "ms";
console.log(ms("2 days"), ms("1h"), ms("-3 hours"), ms("100"));
console.log(ms(60000), ms(2 * 86400000), ms(-3 * 3600000));
console.log(ms(60000, { long: true }), ms(1000, { long: true }));
