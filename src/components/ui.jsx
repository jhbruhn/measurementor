// ── Shared UI primitives ───────────────────────────────────────────────────

export function Input(props) {
  return (
    <input
      {...props}
      className={
        'w-full text-sm border border-gray-200 rounded px-2 py-1.5 ' +
        'focus:outline-none focus:border-green-400 ' +
        'focus-visible:ring-2 focus-visible:ring-green-500 focus-visible:ring-offset-1 ' +
        'bg-white ' +
        (props.className || '')
      }
    />
  );
}

export function Select({ children, ...props }) {
  return (
    <select
      {...props}
      className={
        'w-full text-sm border border-gray-200 rounded px-2 py-1.5 bg-white ' +
        'focus:outline-none focus:border-green-400 ' +
        'focus-visible:ring-2 focus-visible:ring-green-500 focus-visible:ring-offset-1 ' +
        (props.className || '')
      }
    >
      {children}
    </select>
  );
}

export function Btn({ variant = 'default', full, disabled, onClick, title, children, className = '' }) {
  const base =
    'text-sm font-medium rounded px-3 py-1.5 transition-colors disabled:opacity-50 ' +
    'focus:outline-none focus-visible:ring-2 focus-visible:ring-green-500 focus-visible:ring-offset-1 ';
  const variants = {
    default: 'bg-gray-100 hover:bg-gray-200 text-gray-700 border border-gray-200',
    primary: 'bg-green-600 hover:bg-green-700 text-white',
    danger:  'bg-red-600 hover:bg-red-700 text-white',
    ghost:   'text-gray-400 hover:text-red-500 px-1 py-0.5',
  };
  return (
    <button
      onClick={onClick} title={title} disabled={disabled}
      className={base + variants[variant] + (full ? ' w-full' : '') + ' ' + className}
    >
      {children}
    </button>
  );
}

export function Card({ children, className = '' }) {
  return (
    <div className={'rounded-lg border border-gray-200 bg-white p-3 flex flex-col gap-2 ' + className}>
      {children}
    </div>
  );
}

export function CardTitle({ children }) {
  return (
    <span className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
      {children}
    </span>
  );
}

export function Label({ children }) {
  return <label className="block text-xs text-gray-500 mb-0.5">{children}</label>;
}
