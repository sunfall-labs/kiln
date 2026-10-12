type Row = { quantity: number; price: number };
const rows: Row[] = [
  { quantity: 3, price: 40 },
  { quantity: 2, price: 80 },
  { quantity: 1, price: 15 },
];
const totals: number[] = rows.parallelMap(row => row.quantity * row.price);
console.log(totals.parallelFilter(total => total > 100)); // [120, 160]
