const mk = (tag) => { class B { _tag = tag; f = () => tag } return B };
const b = new (mk('T'))();
console.log(b._tag, b.f());
