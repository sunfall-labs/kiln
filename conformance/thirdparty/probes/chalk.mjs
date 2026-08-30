import chalk from "chalk";

// Chalk builds its styles out of proxies and getters over a prototype chain, and
// every style is a callable object -- so it exercises the parts of the object model
// that plain data libraries never reach.
chalk.level = 3;
console.log(chalk.red("red"), chalk.bold.underline("both"));
console.log(chalk.hex("#4488ff")("hex"), chalk.bgGreen.black("bg"));
console.log(chalk.red`tagged ${1 + 1}`, typeof chalk.blue, chalk.blue.name);
