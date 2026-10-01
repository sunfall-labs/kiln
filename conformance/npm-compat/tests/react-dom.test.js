// kiln npm functional test: react-dom
const React = require('react');
const { renderToStaticMarkup, renderToString } = require('react-dom/server');

// 1. Context API with Provider & useContext
const ThemeContext = React.createContext('light');
const UserContext = React.createContext(null);

function ThemedButton(props) {
  const theme = React.useContext(ThemeContext);
  return React.createElement('button', {
    className: 'btn btn-' + theme,
    disabled: props.disabled,
    'data-testid': 'themed-btn'
  }, props.children);
}

function UserCard() {
  const user = React.useContext(UserContext);
  const theme = React.useContext(ThemeContext);
  return React.createElement('div', {
    className: 'card',
    style: { backgroundColor: theme === 'dark' ? '#000' : '#fff', padding: 8 }
  }, React.createElement('h3', null, user ? user.name : 'Anonymous'));
}

// 2. Class Component with lifecycles/render
class CounterBadge extends React.Component {
  render() {
    return React.createElement('span', { className: 'badge' }, 'Count: ' + this.props.count);
  }
}

// 3. Functional Component with useMemo and hooks
function MetricSummary({ items }) {
  const total = React.useMemo(() => {
    return items.reduce((sum, item) => sum + item.value, 0);
  }, [items]);

  return React.createElement(React.Fragment, null,
    React.createElement('p', null, 'Total: ' + total),
    React.createElement('ul', null,
      items.map(item => React.createElement('li', { key: item.id }, item.label + ': ' + item.value))
    )
  );
}

// 4. HTML Escaping & Sanitization
function UnsafeContent() {
  return React.createElement('div', { title: '\"quotes\" & <tags>' },
    '<script>alert(1)</script> & special \"chars\"'
  );
}

// 5. Root Application rendering
function App() {
  const items = [
    { id: 1, label: 'CPU', value: 45 },
    { id: 2, label: 'Memory', value: 80 }
  ];

  return React.createElement('main', { id: 'root' },
    React.createElement(ThemeContext.Provider, { value: 'dark' },
      React.createElement(UserContext.Provider, { value: { name: 'Ada Lovelace' } },
        React.createElement(ThemedButton, { disabled: true }, 'Submit'),
        React.createElement(UserCard, null),
        React.createElement(CounterBadge, { count: 42 }),
        React.createElement(MetricSummary, { items }),
        React.createElement(UnsafeContent, null)
      )
    )
  );
}

const staticHtml = renderToStaticMarkup(React.createElement(App));
console.log('static-html:');
console.log(staticHtml);

const ssrString = renderToString(React.createElement(App));
console.log('ssr-string-starts-with-main:', ssrString.startsWith('<main id=\"root\">'));
console.log('ssr-string-length:', ssrString.length > 0);
