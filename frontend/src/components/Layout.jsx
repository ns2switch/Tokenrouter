const pages = [
  { id: 'dashboard', label: 'Dashboard' },
  { id: 'keys', label: 'Keys' },
  { id: 'pricing', label: 'Pricing' },
  { id: 'providers', label: 'Providers' },
  { id: 'transactions', label: 'Transactions' },
  { id: 'dead-letter', label: 'Dead Letters' },
  { id: 'health', label: 'Health' },
];

export default function Layout({ page, onNavigate, onLogout, children }) {
  return (
    <div className="min-h-screen bg-slate-100">
      <nav className="bg-slate-900 text-white">
        <div className="max-w-7xl mx-auto px-4 flex items-center justify-between h-14">
          <div className="flex items-center gap-1">
            <span className="font-bold text-lg mr-4">TokenRouter</span>
            {pages.map((p) => (
              <button
                key={p.id}
                onClick={() => onNavigate(p.id)}
                className={`px-3 py-1.5 rounded text-sm transition ${
                  page === p.id ? 'bg-white text-slate-900' : 'hover:bg-slate-700'
                }`}
              >
                {p.label}
              </button>
            ))}
          </div>
          <button onClick={onLogout} className="text-sm text-slate-300 hover:text-white">
            Logout
          </button>
        </div>
      </nav>
      <main className="max-w-7xl mx-auto p-6">{children}</main>
    </div>
  );
}
