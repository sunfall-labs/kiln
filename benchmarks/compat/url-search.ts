const u=new URL('https://example.test/a?x=1&x=2'); console.log(JSON.stringify([u.pathname,u.searchParams.getAll('x')]));
