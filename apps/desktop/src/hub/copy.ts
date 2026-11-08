import {usePreferences} from "../settings/preferences";

const en = {
  // navigation
  schedule: "Scheduled", calendar: "Calendar", email: "Email", research: "Research", compare: "Compare",
  // shared
  loading: "Loading…", retry: "Try again", cancel: "Cancel", save: "Save", delete: "Delete", edit: "Edit", close: "Close",
  loadError: "This couldn't be loaded.", saveError: "This couldn't be saved.", none: "None", back: "Back",
  needAccount: "Connect your email in Settings to read and sort your mail and draft replies with an assistant. Nothing is sent until you confirm each message.", openSettings: "Open account settings",
  // schedule
  scheduleTitle: "Scheduled tasks", scheduleEmpty: "Nothing scheduled yet", scheduleEmptyHelp: "Ask an agent to do something at a set time, like a morning summary. Each run opens its own conversation.",
  newTask: "New scheduled task", taskName: "Name", taskPrompt: "What should the agent do?", taskAgent: "Agent", taskProject: "Project",
  noProject: "No project", repeat: "Repeat", once: "Once", every: "Every few minutes", daily: "Every day", weekly: "On chosen days",
  at: "At", date: "Date", time: "Time", minutes: "Minutes", days: "Days", nextRun: "Next run", lastRun: "Last run", never: "Never",
  runNow: "Run now", running: "Running", enable: "On", disable: "Off", runs: "Recent runs", noRuns: "No runs yet.",
  openConversation: "Open conversation", stateDone: "Done", stateFailed: "Failed", stateRunning: "Running", stateInterrupted: "Interrupted",
  scheduleNote: "Runs ask for approval in the app before doing anything that needs it.",
  weekdays: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
  // calendar
  calendarTitle: "Calendar", today: "Today", newEvent: "New event", eventTitle: "Title", eventStart: "Starts", eventEnd: "Ends", allDay: "All day",
  eventPlace: "Place", eventNotes: "Notes", eventRepeat: "Repeats", repeatNone: "Does not repeat", repeatDaily: "Every day", repeatWeekly: "Every week",
  repeatMonthly: "Every month", repeatYearly: "Every year", noEvents: "Nothing on this day.", importIcs: "Import a calendar file", exportIcs: "Export", sync: "Sync",
  syncing: "Syncing…", synced: (n: number) => `${n} events from the server`, calendarWhere: "Calendar", localCalendar: "On this computer", previous: "Previous month", next: "Next month",
  recurring: "Repeats", importDone: (n: number) => `${n} events imported`,
  // email
  emailTitle: "Email", account: "Account", inbox: "Inbox", noMessages: "No messages.", selectMessage: "Choose a message to read it.", from: "From", to: "To", subject: "Subject", date2: "Date",
  untrustedNote: "Written by someone else. Shown as plain text; nothing in it can act on your computer.", summarize: "Summarize", sortMail: "Sort these", draftReply: "Draft a reply",
  remindMe: "Remind me", markSeen: "Mark as read", flag: "Star", unflag: "Remove star", summary: "Summary", labels: "Labels", addLabel: "Add label", loadMore: "Older messages",
  triage: { urgent: "Urgent", reply: "Needs reply", fyi: "For information", newsletter: "Newsletter", other: "Other" } as Record<string, string>,
  drafts: "Drafts", noDrafts: "No drafts.", draftEdit: "Edit draft", recipients: "To (separate with commas)", body: "Message", sendDraft: "Send…",
  confirmSendTitle: "Send this message?", confirmSendBody: (n: number) => `It goes to ${n} recipient(s) exactly as shown. This can't be undone.`, send: "Send", sent: "Sent", failed: "Not sent",
  changedSince: "The draft changed since you looked at it. Read it again.", hourlyLimit: "Too many messages sent this hour. Try again later.", placeholderRecipient: "Replace the placeholder address before sending.",
  instructions: "What should the reply say? (optional)", newDraft: "New message", working: "Working…", remindAt: "Remind me at", project: "Project", chooseProject: "Choose a project", reminderSet: "Reminder added to Tasks.",
  // research
  researchTitle: "Research", researchQuestion: "What do you want to find out?", depth: "Depth", quick: "Quick", standard: "Standard", deep: "Deep",
  quickHelp: "A few sources", standardHelp: "A balanced look", deepHelp: "Many sources, slower", startResearch: "Start research", researchEmpty: "No research yet",
  researchEmptyHelp: "Ask a question. Vatra searches, reads the pages and writes a report where every claim points to its source.", searchNeeded: "Web research needs a search account.",
  report: "Report", sources: "Sources", plan: "Searches", stop: "Stop", notRead: "Not used", followUp: "Ask a follow-up", followUpHelp: "Answers come from what was already read.",
  ask: "Ask", unverified: "Check this carefully", stateWorking: "Working", sourceRead: "Read", sourceSkipped: "Not relevant", sourceUnreadable: "Couldn't be read",
  // compare
  compareTitle: "Compare models", comparePrompt: "Prompt", compareAgents: "Models to compare (choose 2 to 4; add more models in Settings if you only see one)", runCompare: "Compare", answerLabel: (l: string) => `Answer ${l}`,
  pickBest: "Which is better?", tie: "Tie", noneGood: "Neither", revealed: "These were the models:", voted: "Your vote", history: "Earlier comparisons", compareEmpty: "Ask one question and two models answer side by side. Names stay hidden until you choose, so you judge the answers, not the brands.",
  couldntAnswer: "This model couldn't answer.",
  // accounts
  accountsTitle: "Accounts", accountsHelp: "Connect the services Vatra can use. Passwords stay in your system keychain.", addAccount: "Add account", test: "Test connection", testOk: "Connected.", testFailed: "The connection failed.",
  kinds: { caldav: "Calendar", email: "Email", websearch: "Web search (Brave)", telegram: "Telegram bot" } as Record<string, string>,
  kindHelp: {
    caldav: "A calendar from your provider, such as Nextcloud, Fastmail or iCloud. Sign in with Google, or use an app password.",
    email: "Read and sort mail. Messages are only sent when you confirm each one. Sign in with Google or Microsoft, or use an app password.",
    websearch: "Needed for research. Get a key from Brave Search.",
    telegram: "Get results on your phone and ask short questions. Only your own chat is heard.",
  } as Record<string, string>,
  field: {
    name: "Name", url: "Calendar address", username: "User name", password: "Password", address: "Email address", display_name: "Your name", imap_host: "Incoming server", imap_port: "Port",
    imap_security: "Security", smtp_host: "Outgoing server", smtp_port: "Port", smtp_security: "Security", api_key: "API key", bot_token: "Bot token", chat_id: "Chat number", agent: "Model for answers",
    notify_tasks: "Send scheduled task results",
  } as Record<string, string>,
  secure: "Encrypted", starttls: "Encrypted (STARTTLS)", plain: "Not encrypted (this network only)", keepSaved: "Saved. Leave empty to keep it.", saved: "Saved", remove: "Remove",
  noAccounts: "Nothing connected yet.",
  projectDelete: "Delete project",
  projectDeleteBody: (name: string, n: number) => `Delete "${name}"? Its ${n} conversation(s), documents, notes and tasks are deleted too. Files and folders on your computer are not touched. This can't be undone.`,
  projectDeleteNow: "Delete project",
};

export type HubCopy = typeof en;

const ro: HubCopy = {
  schedule: "Programate", calendar: "Calendar", email: "Email", research: "Cercetare", compare: "Compară",
  loading: "Se încarcă…", retry: "Încearcă din nou", cancel: "Anulează", save: "Salvează", delete: "Șterge", edit: "Editează", close: "Închide",
  loadError: "Nu s-a putut încărca.", saveError: "Nu s-a putut salva.", none: "Niciunul", back: "Înapoi",
  needAccount: "Conectează-ți emailul în Setări, ca un asistent să te ajute să-ți citești și să-ți sortezi poșta și să scrii răspunsuri. Nu se trimite nimic până nu confirmi fiecare mesaj.", openSettings: "Deschide setările conturilor",
  scheduleTitle: "Sarcini programate", scheduleEmpty: "Nimic programat încă", scheduleEmptyHelp: "Cere unui agent să facă ceva la o oră stabilită, de exemplu un rezumat dimineața. Fiecare rulare își deschide propria conversație.",
  newTask: "Sarcină programată nouă", taskName: "Nume", taskPrompt: "Ce trebuie să facă agentul?", taskAgent: "Agent", taskProject: "Proiect",
  noProject: "Fără proiect", repeat: "Repetare", once: "O singură dată", every: "La câteva minute", daily: "În fiecare zi", weekly: "În zilele alese",
  at: "La", date: "Data", time: "Ora", minutes: "Minute", days: "Zile", nextRun: "Următoarea rulare", lastRun: "Ultima rulare", never: "Niciodată",
  runNow: "Rulează acum", running: "Rulează", enable: "Pornit", disable: "Oprit", runs: "Rulări recente", noRuns: "Nicio rulare încă.",
  openConversation: "Deschide conversația", stateDone: "Terminat", stateFailed: "Eșuat", stateRunning: "Rulează", stateInterrupted: "Întrerupt",
  scheduleNote: "Rulările cer aprobare în aplicație înainte de orice acțiune care are nevoie de ea.",
  weekdays: ["Dum", "Lun", "Mar", "Mie", "Joi", "Vin", "Sâm"],
  calendarTitle: "Calendar", today: "Azi", newEvent: "Eveniment nou", eventTitle: "Titlu", eventStart: "Începe", eventEnd: "Se termină", allDay: "Toată ziua",
  eventPlace: "Loc", eventNotes: "Note", eventRepeat: "Se repetă", repeatNone: "Nu se repetă", repeatDaily: "În fiecare zi", repeatWeekly: "În fiecare săptămână",
  repeatMonthly: "În fiecare lună", repeatYearly: "În fiecare an", noEvents: "Nimic în această zi.", importIcs: "Importă un fișier de calendar", exportIcs: "Exportă", sync: "Sincronizează",
  syncing: "Se sincronizează…", synced: (n: number) => `${n} evenimente de pe server`, calendarWhere: "Calendar", localCalendar: "Pe acest calculator", previous: "Luna trecută", next: "Luna viitoare",
  recurring: "Se repetă", importDone: (n: number) => `${n} evenimente importate`,
  emailTitle: "Email", account: "Cont", inbox: "Primite", noMessages: "Niciun mesaj.", selectMessage: "Alege un mesaj ca să-l citești.", from: "De la", to: "Către", subject: "Subiect", date2: "Data",
  untrustedNote: "Scris de altcineva. Afișat ca text simplu; nimic din el nu poate acționa pe calculatorul tău.", summarize: "Rezumă", sortMail: "Sortează-le", draftReply: "Scrie un răspuns",
  remindMe: "Amintește-mi", markSeen: "Marchează citit", flag: "Stea", unflag: "Scoate steaua", summary: "Rezumat", labels: "Etichete", addLabel: "Adaugă etichetă", loadMore: "Mesaje mai vechi",
  triage: { urgent: "Urgent", reply: "Cere răspuns", fyi: "Pentru informare", newsletter: "Newsletter", other: "Altele" },
  drafts: "Ciorne", noDrafts: "Nicio ciornă.", draftEdit: "Editează ciorna", recipients: "Către (separate prin virgulă)", body: "Mesaj", sendDraft: "Trimite…",
  confirmSendTitle: "Trimiți acest mesaj?", confirmSendBody: (n: number) => `Pleacă la ${n} destinatar(i) exact cum este afișat. Nu se poate anula.`, send: "Trimite", sent: "Trimis", failed: "Netrimis",
  changedSince: "Ciorna s-a schimbat de când ai citit-o. Citește-o din nou.", hourlyLimit: "Prea multe mesaje trimise în ultima oră. Încearcă mai târziu.", placeholderRecipient: "Înlocuiește adresa provizorie înainte de trimitere.",
  instructions: "Ce să spună răspunsul? (opțional)", newDraft: "Mesaj nou", working: "Se lucrează…", remindAt: "Amintește-mi la", project: "Proiect", chooseProject: "Alege un proiect", reminderSet: "Reamintirea a fost adăugată la Sarcini.",
  researchTitle: "Cercetare", researchQuestion: "Ce vrei să afli?", depth: "Adâncime", quick: "Rapid", standard: "Standard", deep: "Amănunțit",
  quickHelp: "Câteva surse", standardHelp: "O privire echilibrată", deepHelp: "Multe surse, mai lent", startResearch: "Începe cercetarea", researchEmpty: "Nicio cercetare încă",
  researchEmptyHelp: "Pune o întrebare. Vatra caută, citește paginile și scrie un raport în care fiecare afirmație trimite la sursa ei.", searchNeeded: "Cercetarea pe web are nevoie de un cont de căutare.",
  report: "Raport", sources: "Surse", plan: "Căutări", stop: "Oprește", notRead: "Nefolosită", followUp: "Pune o întrebare în plus", followUpHelp: "Răspunsurile vin din ce s-a citit deja.",
  ask: "Întreabă", unverified: "Verifică cu atenție", stateWorking: "Se lucrează", sourceRead: "Citită", sourceSkipped: "Fără legătură", sourceUnreadable: "Nu s-a putut citi",
  compareTitle: "Compară modele", comparePrompt: "Întrebare", compareAgents: "Modele de comparat (alege 2 până la 4; adaugă mai multe modele în Setări dacă vezi doar unul)", runCompare: "Compară", answerLabel: (l: string) => `Răspunsul ${l}`,
  pickBest: "Care e mai bun?", tie: "Egalitate", noneGood: "Niciunul", revealed: "Acestea au fost modelele:", voted: "Votul tău", history: "Comparații anterioare", compareEmpty: "Pune o întrebare și două modele răspund unul lângă altul. Numele rămân ascunse până alegi, ca să judeci răspunsurile, nu mărcile.",
  couldntAnswer: "Acest model nu a putut răspunde.",
  accountsTitle: "Conturi", accountsHelp: "Conectează serviciile pe care le poate folosi Vatra. Parolele rămân în seiful sistemului.", addAccount: "Adaugă cont", test: "Testează conexiunea", testOk: "Conectat.", testFailed: "Conexiunea a eșuat.",
  kinds: { caldav: "Calendar", email: "Email", websearch: "Căutare web (Brave)", telegram: "Bot Telegram" },
  kindHelp: {
    caldav: "Un calendar de la furnizorul tău, de exemplu Nextcloud, Fastmail sau iCloud. Conectează-te cu Google sau folosește o parolă de aplicație.",
    email: "Citește și sortează poșta. Mesajele pleacă doar când confirmi fiecare în parte. Conectează-te cu Google sau Microsoft, ori folosește o parolă de aplicație.",
    websearch: "Necesar pentru cercetare. Ia o cheie de la Brave Search.",
    telegram: "Primești rezultate pe telefon și pui întrebări scurte. Este ascultat doar chatul tău.",
  },
  field: {
    name: "Nume", url: "Adresa calendarului", username: "Utilizator", password: "Parolă", address: "Adresa de email", display_name: "Numele tău", imap_host: "Server de intrare", imap_port: "Port",
    imap_security: "Securitate", smtp_host: "Server de ieșire", smtp_port: "Port", smtp_security: "Securitate", api_key: "Cheie API", bot_token: "Token bot", chat_id: "Numărul chatului", agent: "Model pentru răspunsuri",
    notify_tasks: "Trimite rezultatele sarcinilor programate",
  },
  secure: "Criptat", starttls: "Criptat (STARTTLS)", plain: "Necriptat (doar în această rețea)", keepSaved: "Salvat. Lasă gol ca să-l păstrezi.", saved: "Salvat", remove: "Elimină",
  noAccounts: "Nimic conectat încă.",
  projectDelete: "Șterge proiectul",
  projectDeleteBody: (name: string, n: number) => `Ștergi „${name}"? Se șterg și cele ${n} conversații, documentele, notițele și sarcinile lui. Fișierele și folderele de pe calculator nu sunt atinse. Nu se poate anula.`,
  projectDeleteNow: "Șterge proiectul",
};

export function hubCopy(): HubCopy {
  return usePreferences.getState().preferences.language === "ro" ? ro : en;
}

/** Subscribing to the language makes a component draw again when it changes. */
export function useHubCopy(): HubCopy {
  return usePreferences((state) => state.preferences.language) === "ro" ? ro : en;
}
