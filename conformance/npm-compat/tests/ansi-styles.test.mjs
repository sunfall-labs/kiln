// kiln eval smoke test (authored for kiln, not from porffor).
import styles from 'ansi-styles';
console.log(JSON.stringify(styles.red.open));
console.log(JSON.stringify(styles.red.close));
console.log(JSON.stringify(styles.bold.open));
console.log(JSON.stringify(styles.color.ansi256(196)));
console.log(JSON.stringify(styles.bgColor.ansi16m(255, 128, 0)));
console.log(JSON.stringify(styles.reset.open));
