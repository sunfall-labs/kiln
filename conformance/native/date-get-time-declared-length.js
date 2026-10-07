console.log(Date.prototype.getTime.length);
const getTime = Function.prototype.call.bind(Date.prototype.getTime);
console.log(getTime(new Date(0)));
