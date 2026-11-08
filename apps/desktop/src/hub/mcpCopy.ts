import {usePreferences} from "../settings/preferences";

const en = {
  title: "Tools for your agents",
  intro: "Pick a tool and press Install. Vatra sets it up and turns it on. You can turn it off or remove it later under Installed MCP servers.",
  search: "Search tools", searchPlaceholder: "What do you want your agent to do? For example: GitHub, files, calendar…",
  results: (n: number) => `${n} tools`, onlyReady: "Only tools Vatra can install in one step", loading: "Loading tools…", none: "Nothing found. Try another word.",
  loadMore: "Show more", loadError: "The list couldn't be loaded. Check your internet connection.", retry: "Try again",
  install: "Install", installed: "Installed", manage: "Manage", manual: "Set up by hand", manualWhy: "This tool needs a manual setup that Vatra can't do in one step.",
  online: "Online service", onComputer: "Runs on your computer", needsKey: "Needs a key",
  addOwn: "Add your own server", addOwnHelp: "For a server that isn't in this list.", hideAdvanced: "Close",
  // sheet
  sheetTitle: (name: string) => `Install ${name}`, by: (who: string) => `by ${who}`,
  whatItDoes: "What happens",
  remoteWhat: (host: string) => `Vatra connects your agents to ${host}. What your agents ask this tool is sent to that service.`,
  packageWhat: (pkg: string) => `Vatra downloads and runs a small program on your computer (${pkg}). It can read and change what your agents let it. Install it only if you trust the publisher.`,
  needsNode: "This needs Node.js. If it is missing, Vatra will tell you after the install.",
  valuesTitle: "Information this tool needs", valuesHelp: "Stored safely in your system keychain, never in a file.",
  technical: "Technical details", cancel: "Cancel", installNow: "Install and turn on", working: "Working…",
  preparing: "Preparing…", saving: "Installing…", approving: "Approving…", enabling: "Turning on…", testing: "Checking that it works…",
  ready: (n: number) => (n === 1 ? "Ready. It offers 1 action." : `Ready. It offers ${n} actions.`), readyNoTools: "Ready.",
  signInNeeded: "Installed. Sign in to finish.", signIn: "Sign in", signInStarted: "A sign-in page opened in your browser.",
  testFailed: (why: string) => `Installed and turned on, but it didn't answer: ${why}`, fixLater: "You can check it under Installed MCP servers.",
  failed: (why: string) => `The install didn't finish: ${why}`, done: "Done", missing: "Fill in every field to continue.",
  already: "Already installed. Installing again replaces it.",
};

export type McpCopy = typeof en;

const ro: McpCopy = {
  title: "Unelte pentru agenții tăi",
  intro: "Alege o unealtă și apasă Instalează. Vatra o pregătește și o pornește. O poți opri sau elimina oricând din Servere MCP instalate.",
  search: "Caută unelte", searchPlaceholder: "Ce vrei să facă agentul? De exemplu: GitHub, fișiere, calendar…",
  results: (n: number) => `${n} unelte`, onlyReady: "Doar uneltele pe care Vatra le instalează dintr-un pas", loading: "Se încarcă uneltele…", none: "Nu s-a găsit nimic. Încearcă alt cuvânt.",
  loadMore: "Arată mai multe", loadError: "Lista nu s-a putut încărca. Verifică conexiunea la internet.", retry: "Încearcă din nou",
  install: "Instalează", installed: "Instalat", manage: "Gestionează", manual: "Configurează manual", manualWhy: "Această unealtă cere o configurare manuală pe care Vatra nu o poate face dintr-un pas.",
  online: "Serviciu online", onComputer: "Rulează pe calculatorul tău", needsKey: "Cere o cheie",
  addOwn: "Adaugă propriul server", addOwnHelp: "Pentru un server care nu e în listă.", hideAdvanced: "Închide",
  sheetTitle: (name: string) => `Instalează ${name}`, by: (who: string) => `de ${who}`,
  whatItDoes: "Ce se întâmplă",
  remoteWhat: (host: string) => `Vatra conectează agenții tăi la ${host}. Ce cer agenții de la această unealtă se trimite acelui serviciu.`,
  packageWhat: (pkg: string) => `Vatra descarcă și rulează un mic program pe calculatorul tău (${pkg}). Poate citi și modifica ce îi permit agenții. Instalează-l doar dacă ai încredere în editor.`,
  needsNode: "Are nevoie de Node.js. Dacă lipsește, Vatra îți spune după instalare.",
  valuesTitle: "Informații de care are nevoie această unealtă", valuesHelp: "Se păstrează în siguranță în seiful sistemului, niciodată într-un fișier.",
  technical: "Detalii tehnice", cancel: "Anulează", installNow: "Instalează și pornește", working: "Se lucrează…",
  preparing: "Se pregătește…", saving: "Se instalează…", approving: "Se aprobă…", enabling: "Se pornește…", testing: "Se verifică dacă merge…",
  ready: (n: number) => (n === 1 ? "Gata. Oferă o acțiune." : `Gata. Oferă ${n} acțiuni.`), readyNoTools: "Gata.",
  signInNeeded: "Instalat. Autentifică-te ca să termini.", signIn: "Autentifică-te", signInStarted: "S-a deschis o pagină de autentificare în browser.",
  testFailed: (why: string) => `Instalat și pornit, dar nu a răspuns: ${why}`, fixLater: "Îl poți verifica în Servere MCP instalate.",
  failed: (why: string) => `Instalarea nu s-a terminat: ${why}`, done: "Gata", missing: "Completează toate câmpurile ca să continui.",
  already: "Deja instalat. Dacă instalezi din nou, îl înlocuiești.",
};

export function useMcpCopy(): McpCopy {
  return usePreferences((state) => state.preferences.language) === "ro" ? ro : en;
}
