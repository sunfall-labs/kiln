// kiln eval smoke test (authored for kiln, not from porffor).
import bcrypt from 'bcryptjs';
const hash = bcrypt.hashSync('secret', 8);
console.log(hash.slice(0, 7));
console.log(bcrypt.compareSync('secret', hash));
console.log(bcrypt.compareSync('wrong', hash));
console.log(bcrypt.getRounds(hash));
console.log(bcrypt.hashSync('other', 8).slice(0, 7));
