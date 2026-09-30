// kiln eval smoke test (authored for kiln, not from porffor).
const React = require('react');
const {renderToString, renderToStaticMarkup} = require('react-dom/server');
console.log(renderToString(React.createElement('h1', null, 'Hello')));
console.log(renderToStaticMarkup(React.createElement('p', {className: 'x'}, 'Hi')));
const list = React.createElement('ul', null, [1, 2].map((n) => React.createElement('li', {key: n}, 'i' + n)));
console.log(renderToStaticMarkup(list));
console.log(typeof require('react-dom').createPortal);
