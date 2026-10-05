// kiln npm functional test: preact-render-to-string
const { h, Fragment, Component, createContext } = require('preact');
const render = require('preact-render-to-string');

// 1. Context API in Preact
const ThemeCtx = createContext('light');

const Header = ({ title }) =>
  h('header', { class: 'site-header' },
    h('h1', null, title),
    h(ThemeCtx.Consumer, null, (theme) => h('span', { class: 'theme-badge' }, 'Theme: ' + theme))
  );

// 2. Class Component
class Sidebar extends Component {
  render() {
    return h('aside', { class: 'sidebar' },
      h('ul', null,
        this.props.links.map((link) => h('li', { key: link.href }, h('a', { href: link.href }, link.text)))
      )
    );
  }
}

// 3. SVG & special attribute rendering
const Icon = () =>
  h('svg', { viewBox: '0 0 24 24', width: 24, height: 24, fill: 'none', stroke: 'currentColor' },
    h('path', { strokeLinecap: 'round', strokeLinejoin: 'round', strokeWidth: 2, d: 'M5 13l4 4L19 7' })
  );

// 4. Form inputs with boolean & value attributes
const Form = () =>
  h('form', { method: 'POST', action: '/submit' },
    h('input', { type: 'text', name: 'user', value: 'ada', readonly: true }),
    h('input', { type: 'checkbox', checked: true, disabled: true }),
    h('select', null,
      h('option', { value: '1', selected: true }, 'One'),
      h('option', { value: '2' }, 'Two')
    )
  );

// 5. DangerouslySetInnerHTML & Escaping
const SafeAndUnsafe = () =>
  h(Fragment, null,
    h('div', { class: 'escaped' }, '<span>& special \"quotes\"</span>'),
    h('div', { dangerouslySetInnerHTML: { __html: '<b>bold</b>' } })
  );

// Full Page composition
const App = () =>
  h(ThemeCtx.Provider, { value: 'dark' },
    h('div', { id: 'app' },
      h(Header, { title: 'Preact SSR' }),
      h(Sidebar, { links: [{ href: '/home', text: 'Home' }, { href: '/about', text: 'About' }] }),
      h(Icon, null),
      h(Form, null),
      h(SafeAndUnsafe, null)
    )
  );

console.log(render(h(App, null)));
