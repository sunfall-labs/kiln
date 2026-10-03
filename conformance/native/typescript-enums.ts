enum Fruits {
  Apple,
  Banana,
  Golden = 7,
  Pear,
  Label = "fruit",
}

console.log(Fruits.Apple, Fruits.Banana, Fruits.Golden, Fruits.Pear, Fruits.Label);
console.log(Fruits[0], Fruits[1], Fruits[7], Fruits[8], Fruits["fruit"]);
