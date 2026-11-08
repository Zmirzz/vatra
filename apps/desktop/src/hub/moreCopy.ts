import {usePreferences} from "../settings/preferences";

const en = {
  // research
  clarifyTitle: "A few questions first", clarifyHelp: "Your answers make the search sharper. Leave any blank, or skip them.", clarifyStart: "Start with these answers", clarifySkip: "Skip and start", clarifying: "Checking your question…",
  agentAuto: "Automatic", lightMode: "Light mode for small models", lightHint: "Fewer and shorter steps, so a small model can finish.", lightAuto: (_reason: string) => "Suggested for this computer, which fits small models: fewer, shorter steps.",
  searchingWith: (names: string) => `Searching with ${names}`, noSearch: "No search is available. Turn on DuckDuckGo or add an account in Settings.",
  roundLabel: (n: number, total: number) => `Round ${n} of up to ${total}`,
  statRounds: (n: number) => (n === 1 ? "1 round" : `${n} rounds`), statRead: (n: number) => (n === 1 ? "1 source read" : `${n} sources read`), statSites: (n: number) => (n === 1 ? "1 site" : `${n} sites`),
  statLight: "Light mode", partialTitle: "Partial report", partialHelp: "The report couldn't be written in full, so these are the notes from each source.",
  kinds: {web: "Web", pdf: "PDF", video: "Video", paper: "Paper"} as Record<string, string>, roundN: (n: number) => `Round ${n}`,
  copy: "Copy", copied: "Copied", export: "Export", exportMarkdown: "Markdown (.md)", exportHtml: "Web page (.html)", saveDocument: "Save as a document", discuss: "Discuss", discussWith: "Discuss with",
  savedDocument: "Saved in Documents.", exported: "Exported.", chooseProject: "Choose a project", openDocuments: "Open Documents", startedConversation: "Conversation started.",
  whySearchFailed: "Why searches failed", statsTitle: "How this went",
  // schedule
  taskType: "What to schedule", typeAgent: "A task for an agent", typeResearch: "A research", researchQuestion: "What to research", researchDepth: "Depth",
  openReport: "Open the report", researchTask: "Research",
  // status
  localServer: "Local model server", healthKinds: {"Web search": "Web search", Calendar: "Calendar", Email: "Email", Telegram: "Telegram", "Phone notifications": "Phone notifications", "Search by meaning": "Search by meaning"} as Record<string, string>,
  statusTitle: "Status", statusHelp: "What works and what doesn't. A part that failed shows why, and you can check it again.", statusOk: "Working", statusDown: "Needs attention", statusUnknown: "Not used yet",
  groups: {accounts: "Accounts", search: "Search", models: "Models"} as Record<string, string>, checkAgain: "Check again", checking: "Checking…", lastChecked: (when: string) => `Last checked ${when}`,
  allGood: "Everything that was used is working.", problems: (n: number) => (n === 1 ? "1 thing needs attention." : `${n} things need attention.`), nothing: "Nothing to show yet.",
  // accounts additions
  searchWithoutAccount: "Search without an account", ddgHelp: "DuckDuckGo works from the start, with no key. It is tried last, after your accounts.", ddgOn: "Use DuckDuckGo",
  provider: "Search service", providers: {brave: "Brave Search", tavily: "Tavily", serper: "Serper (Google)", searxng: "SearXNG (your own server)"} as Record<string, string>,
  serverAddress: "Server address", signature: "Signature for replies", topic: "Topic", server: "Server (leave empty for ntfy.sh)", token: "Access token (optional)",
  kindsExtra: {ntfy: "Phone notifications (ntfy)", embeddings: "Search by meaning"} as Record<string, string>,
  kindHelpExtra: {
    ntfy: "Get reminders and the results of scheduled tasks on your phone with the free ntfy app.",
    embeddings: "Find your notes, documents and research by what they mean, and let agents do the same. Needs an embedding model, such as nomic-embed-text in Ollama.",
  } as Record<string, string>,
  baseUrl: "Server address", embeddingModel: "Embedding model",
  browserTitle: "Read pages that need JavaScript", browserHelp: "Some pages show nothing until their scripts run. Vatra can open them in the Edge or Chrome already on this computer. The scripts of those pages then run, so leave this off unless you need it.",
  browserOn: "Open such pages in a browser", browserMissing: "No Edge or Chrome was found on this computer.",
  memoryTitle: "Search by meaning", memoryHelp: "Your notes, documents, tasks and research reports are indexed in the background.", memoryIndexed: (n: number) => (n === 1 ? "1 item indexed" : `${n} items indexed`),
  memoryPending: (n: number) => (n === 0 ? "Up to date" : n === 1 ? "1 waiting" : `${n} waiting`), memoryModel: (m: string) => `Model: ${m}`, memoryAgain: "Index everything again", memoryAgainDone: "Indexing again in the background.",
  foundByMeaning: "Found by meaning", paletteMeaning: "Looking by meaning…", kindItem: "Document or note", kindResearch: "Research",
  // palette
  paletteTitle: "Go to or do anything", palettePlaceholder: "Type to search conversations, projects, pages and actions…", paletteEmpty: "Nothing matches.", paletteHint: "↑↓ to move · Enter to open · Esc to close",
  paletteOpen: "Search everywhere", groupGo: "Go to", groupActions: "Actions", groupConversations: "Conversations", groupProjects: "Projects", groupSettings: "Settings",
  actionNewConversation: "New conversation", actionNewResearch: "Start a research", actionNewEvent: "Add a calendar event", actionToggleTheme: "Switch light or dark", actionOpenSettings: "Open settings",
};

export type MoreCopy = typeof en;

const ro: MoreCopy = {
  clarifyTitle: "Câteva întrebări înainte", clarifyHelp: "Răspunsurile tale fac căutarea mai precisă. Poți lăsa goale câmpurile sau să le sari.", clarifyStart: "Începe cu aceste răspunsuri", clarifySkip: "Sari peste și începe", clarifying: "Se verifică întrebarea…",
  agentAuto: "Automat", lightMode: "Mod ușor pentru modele mici", lightHint: "Pași mai puțini și mai scurți, ca un model mic să poată termina.", lightAuto: (_reason: string) => "Recomandat pentru acest calculator, care se potrivește modelelor mici: pași mai puțini și mai scurți.",
  searchingWith: (names: string) => `Se caută cu ${names}`, noSearch: "Nu e disponibilă nicio căutare. Pornește DuckDuckGo sau adaugă un cont în Setări.",
  roundLabel: (n: number, total: number) => `Runda ${n} din cel mult ${total}`,
  statRounds: (n: number) => (n === 1 ? "o rundă" : `${n} runde`), statRead: (n: number) => (n === 1 ? "o sursă citită" : `${n} surse citite`), statSites: (n: number) => (n === 1 ? "un site" : `${n} site-uri`),
  statLight: "Mod ușor", partialTitle: "Raport parțial", partialHelp: "Raportul nu a putut fi scris complet, deci acestea sunt notițele din fiecare sursă.",
  kinds: {web: "Web", pdf: "PDF", video: "Video", paper: "Lucrare"}, roundN: (n: number) => `Runda ${n}`,
  copy: "Copiază", copied: "Copiat", export: "Exportă", exportMarkdown: "Markdown (.md)", exportHtml: "Pagină web (.html)", saveDocument: "Salvează ca document", discuss: "Discută", discussWith: "Discută cu",
  savedDocument: "Salvat în Documente.", exported: "Exportat.", chooseProject: "Alege un proiect", openDocuments: "Deschide Documente", startedConversation: "Conversația a început.",
  whySearchFailed: "De ce au eșuat căutările", statsTitle: "Cum a decurs",
  taskType: "Ce programezi", typeAgent: "O sarcină pentru un agent", typeResearch: "O cercetare", researchQuestion: "Ce să cerceteze", researchDepth: "Adâncime",
  openReport: "Deschide raportul", researchTask: "Cercetare",
  localServer: "Server de modele local", healthKinds: {"Web search": "Căutare web", Calendar: "Calendar", Email: "Email", Telegram: "Telegram", "Phone notifications": "Notificări pe telefon", "Search by meaning": "Căutare după sens"} as Record<string, string>,
  statusTitle: "Stare", statusHelp: "Ce funcționează și ce nu. O parte care a eșuat arată de ce, și o poți verifica din nou.", statusOk: "Funcționează", statusDown: "Cere atenție", statusUnknown: "Încă nefolosit",
  groups: {accounts: "Conturi", search: "Căutare", models: "Modele"}, checkAgain: "Verifică din nou", checking: "Se verifică…", lastChecked: (when: string) => `Verificat ultima oară ${when}`,
  allGood: "Tot ce a fost folosit funcționează.", problems: (n: number) => (n === 1 ? "Un lucru cere atenție." : `${n} lucruri cer atenție.`), nothing: "Nimic de arătat încă.",
  searchWithoutAccount: "Căutare fără cont", ddgHelp: "DuckDuckGo merge de la început, fără cheie. Este încercat ultimul, după conturile tale.", ddgOn: "Folosește DuckDuckGo",
  provider: "Serviciu de căutare", providers: {brave: "Brave Search", tavily: "Tavily", serper: "Serper (Google)", searxng: "SearXNG (serverul tău)"},
  serverAddress: "Adresa serverului", signature: "Semnătură pentru răspunsuri", topic: "Subiect (topic)", server: "Server (gol pentru ntfy.sh)", token: "Token de acces (opțional)",
  kindsExtra: {ntfy: "Notificări pe telefon (ntfy)", embeddings: "Căutare după sens"},
  kindHelpExtra: {
    ntfy: "Primești reamintiri și rezultatele sarcinilor programate pe telefon, cu aplicația gratuită ntfy.",
    embeddings: "Găsești notițele, documentele și cercetările după ce înseamnă, iar agenții pot face la fel. Are nevoie de un model de embeddings, de exemplu nomic-embed-text în Ollama.",
  },
  baseUrl: "Adresa serverului", embeddingModel: "Model de embeddings",
  browserTitle: "Citește pagini care cer JavaScript", browserHelp: "Unele pagini nu arată nimic până nu rulează scripturile lor. Vatra le poate deschide în Edge sau Chrome, deja instalate pe acest calculator. Scripturile acelor pagini rulează atunci, deci lasă oprit dacă nu ai nevoie.",
  browserOn: "Deschide astfel de pagini într-un browser", browserMissing: "Nu s-a găsit Edge sau Chrome pe acest calculator.",
  memoryTitle: "Căutare după sens", memoryHelp: "Notițele, documentele, sarcinile și rapoartele de cercetare sunt indexate în fundal.", memoryIndexed: (n: number) => (n === 1 ? "un element indexat" : `${n} elemente indexate`),
  memoryPending: (n: number) => (n === 0 ? "La zi" : n === 1 ? "unul în așteptare" : `${n} în așteptare`), memoryModel: (m: string) => `Model: ${m}`, memoryAgain: "Indexează totul din nou", memoryAgainDone: "Se indexează din nou, în fundal.",
  foundByMeaning: "Găsite după sens", paletteMeaning: "Se caută după sens…", kindItem: "Document sau notiță", kindResearch: "Cercetare",
  paletteTitle: "Mergi oriunde sau fă orice", palettePlaceholder: "Scrie ca să cauți conversații, proiecte, pagini și acțiuni…", paletteEmpty: "Nu se potrivește nimic.", paletteHint: "↑↓ ca să te muți · Enter ca să deschizi · Esc ca să închizi",
  paletteOpen: "Caută oriunde", groupGo: "Mergi la", groupActions: "Acțiuni", groupConversations: "Conversații", groupProjects: "Proiecte", groupSettings: "Setări",
  actionNewConversation: "Conversație nouă", actionNewResearch: "Începe o cercetare", actionNewEvent: "Adaugă un eveniment în calendar", actionToggleTheme: "Comută luminos sau întunecat", actionOpenSettings: "Deschide setările",
};

export function useMore(): MoreCopy {
  return usePreferences((state) => state.preferences.language) === "ro" ? ro : en;
}
