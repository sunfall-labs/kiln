function t(l,f){ try { console.log(l,"=>",JSON.stringify(f())); } catch(e){ console.log(l,"=>",e.name+": "+e.message); } }
t("escape obj", ()=> escape({toString(){ throw new RangeError("esc"); }}));
t("unescape obj", ()=> unescape({toString(){ throw new RangeError("unesc"); }}));
t("escape sym", ()=> escape(Symbol("s")));
t("escape ok", ()=> escape("a b"));
t("unescape ok", ()=> unescape("a%20b"));
t("bold sym arg", ()=> "x".bold(Symbol("s")));
t("anchor sym arg", ()=> "x".anchor(Symbol("s")));
t("escape num", ()=> escape(1.5));
