function t(l,f){ try { console.log(l,"=>",JSON.stringify(f())); } catch(e){ console.log(l,"=>",e.name+": "+e.message); } }
t("anchor", ()=> "x".anchor("y"));
t("anchor quote", ()=> "x".anchor('a"b'));
t("bold", ()=> "x".bold());
t("call on number", ()=> String.prototype.anchor.call(5, "y"));
t("call on obj", ()=> String.prototype.anchor.call({toString(){return "o"}}, "y"));
t("call on null", ()=> String.prototype.anchor.call(null, "y"));
t("attr toString throws", ()=> "x".anchor({toString(){ throw new RangeError("attr"); }}));
t("this toString throws", ()=> String.prototype.bold.call({toString(){ throw new RangeError("this"); }}));
t("fontsize", ()=> "x".fontsize(3));
t("link", ()=> "x".link("u"));
t("sub/sup", ()=> "x".sub()+"x".sup());
