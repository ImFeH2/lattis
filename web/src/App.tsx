import "@/style/App.css";

function App() {
  return (
    <div className="flex min-h-screen flex-col bg-stone-950 text-stone-100">
      <nav className="border-b border-stone-800 px-6 py-4">
        <a className="text-lg font-semibold" href="/">
          Lattis
        </a>
      </nav>
      <main className="flex flex-1 flex-col items-center justify-center px-6 py-12 text-center">
        <h1 className="text-4xl font-bold tracking-tight">Lattis</h1>
        <p className="mt-4 max-w-xl text-lg leading-8 text-stone-300">
          A connected space where your devices can work together.
        </p>
      </main>
    </div>
  );
}

export default App;
