class A{static s=1; #p=2; get v(){return 3} set v(n){this.n=n} toString(){return "A"}} const a=new A(); a.v=9; console.log(A.s, a.v, a.n, String(a));
