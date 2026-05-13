import { useState } from 'react';

export default function Login({ onLogin }) {
  const [token, setToken] = useState('');
  const [error, setError] = useState('');

  async function handleSubmit(e) {
    e.preventDefault();
    setError('');
    try {
      await onLogin(token);
    } catch (err) {
      setError(err.message);
    }
  }

  return (
    <div className="min-h-screen bg-slate-100 flex items-center justify-center">
      <form onSubmit={handleSubmit} className="bg-white rounded-xl shadow p-8 w-full max-w-sm space-y-4">
        <h1 className="text-2xl font-bold text-center text-slate-900">TokenRouter Admin</h1>
        {error && <p className="text-red-600 text-sm text-center">{error}</p>}
        <input
          className="border rounded p-3 w-full"
          type="password"
          placeholder="Admin token"
          value={token}
          onChange={(e) => setToken(e.target.value)}
          required
          autoFocus
        />
        <button className="bg-slate-900 text-white rounded p-3 w-full font-semibold" type="submit">
          Login
        </button>
      </form>
    </div>
  );
}
