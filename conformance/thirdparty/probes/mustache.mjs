import Mustache from "mustache";
console.log(Mustache.render("Hello {{name}}! {{#items}}[{{.}}]{{/items}}", { name: "kiln", items: [1, 2] }));
