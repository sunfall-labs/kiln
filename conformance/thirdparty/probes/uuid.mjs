import { v5 as uuidv5, validate, version, parse, stringify } from "uuid";
const id = uuidv5("kiln", uuidv5.DNS);
console.log(id, validate(id), version(id));
console.log(stringify(parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8")));
