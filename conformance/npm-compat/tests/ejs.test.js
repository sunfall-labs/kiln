// kiln eval smoke test (authored for kiln, not from porffor).
const ejs = require('ejs');
console.log(ejs.render('<h1><%= title %></h1>', { title: 'kiln' }));
console.log(ejs.render('<% items.forEach(i => { %>-<%= i %> <% }) %>', { items: [1, 2, 3] }));
console.log(ejs.render('<%= 2 + 3 %>'));
console.log(ejs.render('<%- html %>', { html: '<b>raw</b>' }));
console.log(ejs.render('<% if (show) { %>yes<% } else { %>no<% } %>', { show: true }));
