(function () {
  'use strict';

  var CSS = `
    :host {
      box-sizing: border-box;
      font-family: system-ui, -apple-system, sans-serif;
    }
    :host([hidden]) { display: none !important; }
  `;

  function sheet(extra) {
    var s = new CSSStyleSheet();
    s.replaceSync(CSS + (extra || ''));
    return s;
  }

  /* ── weft-button ─────────────────────────────────────────────── */
  class WeftButton extends HTMLElement {
    static observedAttributes = ['variant', 'disabled'];
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: inline-block; }
        button {
          display: inline-flex; align-items: center; justify-content: center;
          gap: 6px; padding: 8px 16px; border: none; border-radius: 8px;
          font-size: 14px; font-weight: 500; cursor: pointer;
          background: rgba(91,138,245,0.9); color: #fff;
          transition: opacity 0.15s, background 0.15s;
          width: 100%;
        }
        button:hover { background: rgba(91,138,245,1); }
        button:active { opacity: 0.8; }
        button:disabled { opacity: 0.4; cursor: not-allowed; }
        :host([variant=secondary]) button {
          background: rgba(255,255,255,0.1);
          color: rgba(255,255,255,0.9);
        }
        :host([variant=destructive]) button {
          background: rgba(220,50,50,0.85);
        }
      `)];
      this._btn = document.createElement('button');
      this._btn.appendChild(document.createElement('slot'));
      root.appendChild(this._btn);
    }
    attributeChangedCallback(name, _old, val) {
      if (name === 'disabled') this._btn.disabled = val !== null;
    }
    connectedCallback() {
      this._btn.disabled = this.hasAttribute('disabled');
    }
  }

  /* ── weft-card ───────────────────────────────────────────────── */
  class WeftCard extends HTMLElement {
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: block; }
        .card {
          background: rgba(255,255,255,0.07);
          border: 1px solid rgba(255,255,255,0.12);
          border-radius: 12px; padding: 16px;
        }
      `)];
      var d = document.createElement('div');
      d.className = 'card';
      d.appendChild(document.createElement('slot'));
      root.appendChild(d);
    }
  }

  /* ── weft-dialog ─────────────────────────────────────────────── */
  class WeftDialog extends HTMLElement {
    static observedAttributes = ['open'];
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: none; }
        :host([open]) { display: flex; align-items: center; justify-content: center;
          position: fixed; inset: 0; z-index: 9000;
          background: rgba(0,0,0,0.55); }
        .dialog {
          background: #1a1d28; border: 1px solid rgba(255,255,255,0.15);
          border-radius: 16px; padding: 24px; min-width: 320px;
          max-width: 90vw; max-height: 80vh; overflow-y: auto;
          box-shadow: 0 24px 64px rgba(0,0,0,0.6);
        }
      `)];
      var d = document.createElement('div');
      d.className = 'dialog';
      d.appendChild(document.createElement('slot'));
      root.appendChild(d);
      root.addEventListener('click', function (e) {
        if (e.target === root.host) root.host.removeAttribute('open');
      });
    }
  }

  /* ── weft-icon ───────────────────────────────────────────────── */
  class WeftIcon extends HTMLElement {
    static observedAttributes = ['name', 'size'];
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: inline-flex; align-items: center; justify-content: center; }
        svg { width: var(--icon-size, 20px); height: var(--icon-size, 20px);
              fill: currentColor; }
      `)];
      this._root = root;
      this._render();
    }
    attributeChangedCallback() { this._render(); }
    _render() {
      var size = this.getAttribute('size') || '20';
      this._root.host.style.setProperty('--icon-size', size + 'px');
    }
  }

  /* ── weft-list / weft-list-item ─────────────────────────────── */
  class WeftList extends HTMLElement {
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: block; }
        ul { list-style: none; margin: 0; padding: 0; }
      `)];
      var ul = document.createElement('ul');
      ul.appendChild(document.createElement('slot'));
      root.appendChild(ul);
    }
  }

  class WeftListItem extends HTMLElement {
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: block; }
        li {
          display: flex; align-items: center; gap: 10px;
          padding: 10px 12px; border-radius: 8px; cursor: pointer;
          color: rgba(255,255,255,0.88);
          transition: background 0.12s;
        }
        li:hover { background: rgba(255,255,255,0.08); }
      `)];
      var li = document.createElement('li');
      li.appendChild(document.createElement('slot'));
      root.appendChild(li);
    }
  }

  /* ── weft-menu / weft-menu-item ─────────────────────────────── */
  class WeftMenu extends HTMLElement {
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: block; }
        .menu {
          background: rgba(20,22,32,0.95); backdrop-filter: blur(20px);
          border: 1px solid rgba(255,255,255,0.12); border-radius: 10px;
          padding: 4px; min-width: 160px;
          box-shadow: 0 8px 32px rgba(0,0,0,0.4);
        }
      `)];
      var d = document.createElement('div');
      d.className = 'menu';
      d.appendChild(document.createElement('slot'));
      root.appendChild(d);
    }
  }

  class WeftMenuItem extends HTMLElement {
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: block; }
        .item {
          display: flex; align-items: center; gap: 8px;
          padding: 8px 12px; border-radius: 6px; cursor: pointer;
          font-size: 13px; color: rgba(255,255,255,0.88);
          transition: background 0.1s;
        }
        .item:hover { background: rgba(91,138,245,0.25); }
        :host([destructive]) .item { color: #f87171; }
      `)];
      var d = document.createElement('div');
      d.className = 'item';
      d.appendChild(document.createElement('slot'));
      root.appendChild(d);
    }
  }

  /* ── weft-progress ───────────────────────────────────────────── */
  class WeftProgress extends HTMLElement {
    static observedAttributes = ['value', 'max', 'indeterminate'];
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: block; }
        .track {
          height: 6px; background: rgba(255,255,255,0.12);
          border-radius: 3px; overflow: hidden;
        }
        .fill {
          height: 100%; background: #5b8af5; border-radius: 3px;
          transition: width 0.2s;
        }
        @keyframes indeterminate {
          0%   { transform: translateX(-100%); }
          100% { transform: translateX(400%); }
        }
        :host([indeterminate]) .fill {
          width: 25%; animation: indeterminate 1.4s linear infinite;
        }
      `)];
      this._track = document.createElement('div');
      this._track.className = 'track';
      this._fill = document.createElement('div');
      this._fill.className = 'fill';
      this._track.appendChild(this._fill);
      root.appendChild(this._track);
      this._update();
    }
    attributeChangedCallback() { this._update(); }
    _update() {
      if (this.hasAttribute('indeterminate')) {
        this._fill.style.width = '25%';
      } else {
        var val = parseFloat(this.getAttribute('value') || '0');
        var max = parseFloat(this.getAttribute('max') || '100');
        this._fill.style.width = (Math.min(100, (val / max) * 100)) + '%';
      }
    }
  }

  /* ── weft-input ──────────────────────────────────────────────── */
  class WeftInput extends HTMLElement {
    static observedAttributes = ['placeholder', 'type', 'value', 'disabled'];
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: block; }
        input {
          width: 100%; padding: 9px 12px; border-radius: 8px;
          border: 1px solid rgba(255,255,255,0.15);
          background: rgba(255,255,255,0.07);
          color: rgba(255,255,255,0.92); font-size: 14px;
          outline: none; transition: border-color 0.15s;
        }
        input::placeholder { color: rgba(255,255,255,0.35); }
        input:focus { border-color: rgba(91,138,245,0.7); }
        input:disabled { opacity: 0.45; cursor: not-allowed; }
      `)];
      this._input = document.createElement('input');
      root.appendChild(this._input);
      this._input.addEventListener('input', function (e) {
        this.dispatchEvent(new CustomEvent('weft:input', { detail: e.target.value, bubbles: true }));
      }.bind(this));
      this._sync();
    }
    attributeChangedCallback() { this._sync(); }
    _sync() {
      var i = this._input;
      if (!i) return;
      i.placeholder = this.getAttribute('placeholder') || '';
      i.type = this.getAttribute('type') || 'text';
      if (this.hasAttribute('value')) i.value = this.getAttribute('value');
      i.disabled = this.hasAttribute('disabled');
    }
    get value() { return this._input ? this._input.value : ''; }
    set value(v) { if (this._input) this._input.value = v; }
  }

  /* ── weft-label ──────────────────────────────────────────────── */
  class WeftLabel extends HTMLElement {
    constructor() {
      super();
      var root = this.attachShadow({ mode: 'open' });
      root.adoptedStyleSheets = [sheet(`
        :host { display: inline-block; }
        .label {
          display: inline-flex; align-items: center; gap: 4px;
          padding: 2px 8px; border-radius: 100px; font-size: 11px;
          font-weight: 600; letter-spacing: 0.02em;
          background: rgba(91,138,245,0.2); color: #93b4ff;
        }
        :host([variant=success]) .label { background: rgba(52,199,89,0.2); color: #6ee09c; }
        :host([variant=warning]) .label { background: rgba(255,159,10,0.2); color: #ffd060; }
        :host([variant=error]) .label { background: rgba(255,69,58,0.2); color: #ff8a80; }
        :host([variant=neutral]) .label { background: rgba(255,255,255,0.1); color: rgba(255,255,255,0.7); }
      `)];
      var d = document.createElement('div');
      d.className = 'label';
      d.appendChild(document.createElement('slot'));
      root.appendChild(d);
    }
  }

  /* ── registration ────────────────────────────────────────────── */
  var defs = {
    'weft-button':    WeftButton,
    'weft-card':      WeftCard,
    'weft-dialog':    WeftDialog,
    'weft-icon':      WeftIcon,
    'weft-list':      WeftList,
    'weft-list-item': WeftListItem,
    'weft-menu':      WeftMenu,
    'weft-menu-item': WeftMenuItem,
    'weft-progress':  WeftProgress,
    'weft-input':     WeftInput,
    'weft-label':     WeftLabel,
  };

  Object.keys(defs).forEach(function (name) {
    if (!customElements.get(name)) {
      customElements.define(name, defs[name]);
    }
  });
}());
