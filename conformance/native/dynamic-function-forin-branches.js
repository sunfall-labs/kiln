const visit = new Function(
  "items",
  "var output = []; for (var key in items) if (key) { if (items[key]) output.push(key); else output.push('!' + key); } return output.join(',');",
);

console.log(visit({ first: true, second: false }));
