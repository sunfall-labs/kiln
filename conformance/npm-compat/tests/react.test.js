// kiln eval smoke test (authored for kiln, not from porffor).
const React = require('react');

const el = React.createElement('div', { className: 'a' }, 'hello');
console.log(React.isValidElement(el));
console.log(el.type, JSON.stringify(el.props.children));

console.log(React.Children.count([el, el, el]));
console.log(React.isValidElement('not-an-element'));

const frag = React.createElement(React.Fragment, null, el);
console.log(React.isValidElement(frag));
console.log(JSON.stringify(React.version));
