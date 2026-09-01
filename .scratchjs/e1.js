function t(l,f){ try { f(); console.log(l,"OK"); } catch(e){ console.log(l, e.constructor.name); } }
t("body var arguments", function(){ (function(){ eval("var arguments"); })(); });
t("param default var arguments", function(){ (function(p = eval("var arguments")){})(); });
t("param default var x, no conflict", function(){ (function(p = eval("var zz1")){})(); });
t("body let x then var x", function(){ (function(){ let x; eval("var x"); })(); });
t("body var x then var x", function(){ (function(){ var x; eval("var x"); })(); });
t("param default var p (param name)", function(){ (function(p = eval("var p")){})(); });
t("param default var q, q is later param", function(){ (function(p = eval("var q"), q){})(); });
t("block let then var", function(){ (function(){ { let y; eval("var y"); } })(); });
