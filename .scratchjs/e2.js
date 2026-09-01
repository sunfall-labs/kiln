function t(l,f){ try { f(); console.log(l,"NO-THROW"); } catch(e){ console.log(l, e.constructor.name); } }
async function * g1(p = eval("var arguments")) {}
t("asyncgen decl", g1);
const a1 = (p = eval("var arguments = 'param'"), arguments) => {};
t("arrow", a1);
function f1(p = eval("var arguments")) {}
t("plain fn decl", f1);
var f2 = function(p = eval("var arguments")) {};
t("fn expr", f2);
function* g2(p = eval("var arguments")) {}
t("generator", g2);
async function a2(p = eval("var arguments")) {}
t("async fn", a2);
