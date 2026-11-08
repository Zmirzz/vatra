# Model de amenințări: Vatra

Data: 2026-11-04. Branch: `plan-e1-foundation`.

Documentul descrie apărările găsite în cod și limitele lor. Referințele sunt căi relative la repository și nume de funcții. Este o analiză statică, nu un test de penetrare; o apărare din cod nu dovedește comportamentul tuturor programelor externe.

## 1. Active, actori și granițe

| Activ | Unde stă | Granița de încredere |
|---|---|---|
| Fișierele utilizatorului | proiecte și fișiere accesibile uneltelor | model → aprobări → unelte → sistem de fișiere |
| Cheile agenților configurați în aplicație | Windows keychain, `vatra/agent/<name>`; doar setările în SQLite | formular → RPC autentificat → `setup::save` → `SecretStore` |
| Cheile din configurația avansată | variabilele `api_key_env` | mediul serviciului → driverul modelului |
| Secrete MCP, tokenuri HF și ChatGPT | keychain; în memorie când sunt folosite | serviciu → furnizor sau proces autorizat |
| Tokenul serviciului | `%APPDATA%\Vatra\token` | profil Windows → desktop → WebSocket pe loopback |
| Istoric și rezultate de verificare | `%APPDATA%\Vatra\vatra.db`, SQLite necriptat | serviciu → disc local |
| Modele | `%LOCALAPPDATA%\Vatra\models` | Hugging Face → descărcător → parser GGUF → llama.cpp |
| Cheia de actualizare | cheia privată în afara repo-ului; cheia publică în configurația Tauri | procesul de release → server → updater |
| Conturi pe abonament | sesiunile native Claude/Codex și conexiunea ChatGPT a Vatra | Vatra → binarul/adaptorul ales → furnizor |

Actori: o pagină web care poate contacta loopback; un proces local al aceluiași utilizator; alt utilizator Windows; un MCP/plugin importat; un repo sau un model malițios; un model care urmează instrucțiuni injectate; un furnizor sau un server de distribuție compromis. Procesele aceluiași utilizator au acces larg la profil și keychain; Vatra nu încearcă să izoleze aplicațiile acelui utilizator între ele.

## 2. Suprafețe și apărări

### 2.1 Tokenul serviciului și RPC

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| O pagină web apelează serviciul | `crates/vatra-service/src/rpc.rs: upgrade` cere protocolul `vatra.bearer.<token>` și verifică originea. `origin_allowed` permite `tauri://localhost` și `http://tauri.localhost`; originea Vite `http://localhost:1420` este permisă numai când `cfg!(debug_assertions)` este adevărat. | O cerere fără `Origin` este acceptată dacă are tokenul. Orice cod compromis în webview poate folosi tokenul pe care desktopul îl primește. |
| Token ghicit sau refolosit | `crates/vatra-service/src/runtime.rs: new_token` generează 32 de octeți aleatori la pornire; `Runtime::start` ia lock pe `service.lock`; `bridge.rs: token_matches` compară fără ieșire timpurie. | Fișierul token este scris în profil cu ACL moștenit, fără ACL explicit. Nu există protecție față de alt proces al aceluiași utilizator. |
| Apel intern fără autentificare | `bridge.rs: internal_router` și `mcp_proxy.rs: internal_mcp_router` cer bearer token. | `/health` este public pe loopback și expune pid-ul; identificarea serviciului prin health nu autentifică criptografic procesul. |
| Consum de resurse | `rpc.rs: MAX_MESSAGE_BYTES` limitează mesajul la 16 MiB. | Nu există o limită globală de conexiuni. Tokenul vechi poate rămâne pe disc după oprire. |

### 2.2 Chei API, keychain și conturi

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| Cheia introdusă în aplicație ajunge în TOML sau SQLite | `crates/vatra-service/src/setup.rs: agent_from_form` returnează cheia separat de `settings_json`; `save` o scrie prin `SecretStore`. `config.rs: AgentConfig::Model.api_key` are `#[serde(skip)]`; `Config::merged_with` citește cheia din keychain. | Cheia există în memoria formularului și a serviciului; un proces al aceluiași utilizator poate accesa keychain-ul. Salvarea keychain + SQLite nu este o singură tranzacție. |
| Chei de mediu moștenite de shell, MCP sau adaptoare | `agents.rs: hidden_names` include `api_key_env` și cheia de căutare; `hidden_env` este transmis uneltelor și adaptoarelor. `crates/vatra-engine/src/tools/bash.rs: finish_command` și `crates/vatra-acp/src/transport.rs` scot variabilele ascunse și `VATRA_API_KEY`; `crates/vatra-claude/src/adapter.rs` scoate `removed_env`. | Variabile sensibile care nu sunt configurate ca ascunse pot fi moștenite. Un agent poate citi ulterior secrete din fișiere accesibile. |
| Secrete MCP importate în clar | `crates/vatra-extensions/src/mcp_file.rs: keep_or_report` separă valorile recunoscute drept secrete; `catalog.rs: set_secret` le păstrează în keychain. `crates/vatra-core/src/secrets.rs: ChunkedStore` împarte valorile mari. | Recunoașterea numelor de secrete este euristică; un nume neobișnuit poate trece. |
| Token HF trimis altui host | `crates/vatra-service/src/models/rpc.rs: hf.set_token` păstrează tokenul în keychain; `crates/vatra-models/src/hf.rs: token_for` cere host și port identice cu baza clientului. `ModelManager` public folosește `https://huggingface.co/`; redirecturile HTTP sunt gestionate de reqwest, care elimină Authorization la schimbarea hostului. | Tokenul rămâne în memoria clientului; protecția redirecturilor depinde de reqwest. Baza injectabilă este folosită și în teste; nu este o listă globală hardcodată de hosturi. |
| Conectare ChatGPT interceptată | `crates/vatra-service/src/chatgpt.rs: connect_inner` folosește PKCE S256, state și nonce; `callback` verifică state; `verify_id` verifică RS256, issuer, audience, expirare și nonce. Clientul nu urmărește redirecturi. | Tokenul de acces este transmis intenționat copilului autorizat prin mediu (`ChildAccessToken::apply_to`). |
| Refresh/revocare incompletă | `chatgpt.rs: access_token_for_child` marchează refresh-ul început; `disconnect` șterge credențialele locale chiar dacă revocarea nu este confirmată. | Reînnoirea eșuată poate cere reconectare. Un token nerevocat la furnizor poate rămâne valabil. |
| Importul citește autentificarea nativă | `crates/vatra-import/src/guard.rs: is_forbidden` refuză fișierele de credențiale cunoscute, inclusiv `auth.json` și `.credentials.json`. | Lista nu poate identifica orice fișier secret cu un nume arbitrar. Istoricul SQLite rămâne necriptat. |

### 2.3 Serverul llama.cpp și modelele

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| Browserul sau alt proces folosește modelul local | `crates/vatra-service/src/models/server.rs: args` fixează `--host 127.0.0.1`; `spawn` generează o cheie nouă și o pune în `LLAMA_API_KEY`, fără argument de comandă. | Cheia este în mediul copilului. Protecția efectivă depinde de build-ul llama.cpp folosit. |
| Portul este ocupat între alegere și pornire | `server.rs: free_port` cere un port liber sistemului. | Există o fereastră între eliberarea portului și pornire; un proces local care câștigă cursa poate primi o cerere autentificată destinată serverului. |
| Model corupt sau descărcare alterată | `crates/vatra-service/src/models/download.rs: fetch` verifică SHA-256 când există și redenumește `.part` numai după verificare; `manager.rs: safe_join` refuză componente nesigure. | Hash-ul de la același furnizor dovedește integritatea, nu autorul; fișierele fără hash sunt acceptate. `resolve/main` poate schimba conținutul între citiri. |
| GGUF cu lungimi false consumă memorie | `crates/vatra-models/src/gguf.rs` limitează antetul la 64 MiB, numărul de valori, adâncimea și elementele păstrate. | Parserul llama.cpp și șablonul Jinja al modelului nu sunt izolate de aceste limite Rust. |
| Binar din PATH înlocuit | `crates/vatra-models/src/llama.rs: find` preferă calea configurată, apoi PATH și winget. | Binarul găsit este executat fără verificare proprie de hash sau semnătură. |
| Server rămas pornit | `server.rs: watch_idle`, `kill_on_drop` și `crates/vatra-core/src/process.rs: ProcessGroup` închid serverul și procesele adoptate. | Curățarea proceselor nu este sandbox. |

### 2.4 MCP-uri, aprobări și unelte

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| Cod importat activat sau schimbat fără aprobare | `crates/vatra-extensions/src/catalog.rs: approve` verifică expected_hash; `is_approved` și `active_for` compară și recalculează hash-ul copiei. | Aprobarea hash-ului nu verifică comportamentul codului. O schimbare după încărcare rămâne posibilă. |
| Importul modifică sursele Claude/Codex | `crates/vatra-import/src/guard.rs: GuardedFs` expune citiri, verifică căile canonice și nu urmează legăturile la traversare. | Nu este o apărare generală contra proceselor externe care modifică aceleași fișiere. |
| Modelul scrie în afara proiectului | `crates/vatra-engine/src/permissions.rs: mode_decision` cere aprobare pentru `EditOutside`, exceptând FullAccess. Uneltele Edit și Write calculează clasa căii. | FullAccess permite acțiunea. Citirea în proiect nu exclude implicit `.env` sau chei. |
| Comandă injectată sau exfiltrare | `permissions.rs: decide` aplică deny, ask, allow și modul; `ApprovalCard.tsx` arată argumentele. `web_fetch.rs` verifică adresele private și urmărește redirecturile manual. | Regulile sunt pe operații/text, nu izolare OS. MCP-urile HTTP primesc datele trimise; un MCP fără cod local poate fi activ fără aprobarea unui hash. |
| Cod în rezultatele modelului ajunge în webview | `apps/desktop/src/components/Markdown.tsx` nu redă HTML brut, filtrează schemele linkurilor și transformă imaginile externe în linkuri; CSP limitează sursele. | Prompt injection poate determina acțiuni permise sau aprobate. Conținutul sursă nu primește o graniță de încredere impusă de model. |
| Procese supraviețuiesc turei | `crates/vatra-core/src/process.rs: ProcessGroup` folosește Job Object cu `KILL_ON_JOB_CLOSE` pe Windows. | Copilul rulează cu drepturile utilizatorului. Adoptarea și curățarea nu restricționează fișierele sau rețeaua. |

### 2.5 Instalarea dependențelor prin winget

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| Un ID arbitrar este instalat | `crates/vatra-service/src/deps.rs: PACKAGES`, `package` și `install` permit doar cele cinci ID-uri fixe. `install_args` folosește argumente separate, `--exact`, `--scope user`, `--silent` și `--disable-interactivity`. | Versiunea nu este fixată; sursa winget, manifestul și instalatorul pachetului sunt în granița de încredere. |
| Instalare pornită fără înțelegerea efectului | `apps/desktop/src/components/DependencyDialog.tsx` prezintă sursa, comanda și acceptarea licenței înainte de apelul `deps.install`. | RPC-ul autentificat este operația autorizată; nu există o dovadă criptografică de click sau token separat de consimțământ. Orice client cu tokenul poate cere instalarea unui pachet permis. |
| winget se blochează sau eșuează | `deps.rs: install` are timeout de 15 minute, stdin închis, ultimele 12 linii, notificări și `ProcessGroup`; la timeout se termină arborele adoptat. Testul cu instalator fals reproduce un descendent care supraviețuia înaintea fixului. | Job Object nu promite rollback al unei instalări parțiale și nu poate anula efectele deja produse de instalator. |
| Programul nou nu este găsit | `deps.rs: fresh_path` citește PATH din HKLM/HKCU; `apply_fresh_path` îl aplică și handlerul redetectează llama.cpp. | PATH și registry pot fi modificate de utilizator; căutarea binarelor nu validează autenticitatea lor. |

### 2.6 Instalator și actualizări

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| Instalatorul cere privilegii globale | `apps/desktop/src-tauri/tauri.conf.json: bundle.windows.nsis.installMode` este `currentUser`. | Instalatorul nu este semnat Authenticode. Rularea efectivă instalare/dezinstalare cere validare separată; configurația nu o dovedește. |
| Server de actualizare compromis | Tauri inițializează `tauri-plugin-updater` în `apps/desktop/src-tauri/src/lib.rs`; cheia publică este în `tauri.conf.json`. Pluginul verifică semnătura artefactului la instalare. | `lib.rs: install_update` apelează `download_and_install` după confirmarea din interfață. Verificarea manifestului de disponibilitate singură nu este verificarea semnăturii unui artefact; testul offline `tests/update_signature.rs` acoperă numai primitiva minisign, nu un endpoint și un instalator real. |
| Verificări sau descărcări neașteptate | `plugins.updater.endpoints` este gol; `update_status` tratează `EmptyEndpoints` ca neconfigurat. | Înainte de publicare trebuie definit endpoint HTTPS și testată calea completă descărcare/verificare/instalare cu un artefact de test. |
| Cheia de semnare este pierdută sau furată | `docs/release.md` cere păstrarea cheii private în afara repository-ului și backup. | Posesorul cheii poate semna actualizări. O cheie nouă cere migrare/reinstalare; nu există serviciu public de release în acest branch. |

### 2.7 Agenți pe abonament și verificările de paritate

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| Binarul nativ Claude/Codex este modificat | `crates/vatra-claude/src/adapter.rs` construiește argumente și rezolvă binarul; `chatgpt.rs: app_server_command` pregătește lansarea. Configurațiile native nu sunt rescrise pentru autentificarea Vatra. | Binarul și adaptorul nu sunt verificate prin hash de Vatra. O rulare reală poate consuma abonament sau bani și necesită consimțământ separat. |
| Codex folosește chei API accidentale | `chatgpt.rs: app_server_command` elimină `OPENAI_API_KEY` și `CODEX_API_KEY` pentru ruta ChatGPT. | Tokenul accesului autorizat există în mediul copilului. |
| Evaluarea unei sarcini accesează fișiere sau execută comenzi | `crates/vatra-parity/src/task.rs` definește criteriile; `evaluate.rs` evaluează fișiere, unelte și comenzi în copia fixture-ului. Sarcinile și comenzile lor sunt conținut de încredere din repository, nu intrări arbitrare de model. | O copie temporară și aprobarea numai a comenzilor listate nu constituie sandbox. Un script aprobat, modificat de model, poate accesa restul profilului sau rețeaua. |
| Rezultatul scurt este interpretat ca paritate generală | `vatra-service/src/parity.rs` include fixture-urile în binar, creează o copie nouă per sarcină și exportă acoperirea/modelul/build-ul/data. `parity_native.rs` folosește procese supravegheate și configurare temporară a permisiunilor pentru Claude. | Șase sarcini nu dovedesc siguranță sau paritate generală. ACP este refuzat de runner-ul izolat; Codex nativ raportează Not run, fără spawn. Comportamentul hook-ului Claude la timeout nu a fost verificat pe CLI real. |

### 2.8 Acțiuni cu efect în exterior

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| Un mesaj, o invitație sau o ștergere de date remote se face fără știrea persoanei | `crates/vatra-engine/src/permissions.rs: decide` întoarce `Ask` pentru `ToolClass::External` în orice mod. Regulile `allow` și permisiunile acordate pe tură nu sar întrebarea; o regulă `deny` oprește apelul înainte de întrebare. | Nicio unealtă de această clasă nu există încă în aplicație; apărarea se verifică la adăugarea primelor unelte. Întrebarea arată numele uneltei și argumentele complete (`external_gate` apelează `approver.approve(&call.name, arguments)`); `summary` ajunge numai în jurnalul de audit. Uneltele MCP sunt de clasa `Mcp` și nu sunt niciodată `External` astăzi: în E4, o unealtă de email sau calendar prin MCP trebuie mapată la `External` înainte să fie considerată de încredere. |
| Un aprobator care răspunde automat „da” aprobă o acțiune externă | Aprobatorul are metoda `interactive()`. `AllowAll`, folosit de steagul `--yes` al CLI-ului, răspunde „nu este interactiv”: `external_gate` refuză apelul fără să-l ruleze, cu mesajul că acțiunea cere aprobarea persoanei, și scrie o intrare `blocked` când există magazin de audit. Ceilalți aprobatori își păstrează comportamentul. | `--yes` aprobă în continuare toate celelalte clase de unealtă. Un aprobator nou care răspunde „da” fără persoană trebuie să returneze `interactive() == false`; regula nu e impusă de tipuri. |
| Acțiunea aprobată sau refuzată nu rămâne în urmă | `crates/vatra-engine/src/engine.rs: external_gate` scrie intrarea `approved` înainte de execuție; dacă intrarea nu poate fi scrisă, apelul nu rulează. Refuzul și blocarea se înregistrează. `crates/vatra-core/src/audit.rs` ține tabela `audit_log`. | Jurnalul este în SQLite necriptat și poate fi modificat de un proces al aceluiași utilizator. Fără magazin de audit (`audit: None`) acțiunea este întrebată, dar nu se înregistrează și limita nu se aplică; serviciul și subagenții primesc întotdeauna magazinul. |
| Jurnalul ajunge să conțină conținutul mesajului | Jurnalul reține unealta, conectorul, rezumatul din `ExternalAction` (tăiat la 200 de caractere) și SHA-256 al argumentelor (`payload_hash`), niciodată argumentele sau corpul mesajului. | „Fără conținut în jurnale” este adevărat numai pentru `audit_log`: jurnalul de evenimente al sesiunii (`Event::ToolCall`, cu argumentele) păstrează argumentele complete, deci și corpul unui mesaj trimis. Rezumatul îl scrie unealta; o unealtă care pune corpul mesajului în rezumat îl va scrie în jurnal (tăiat la 200 de caractere); regula trebuie verificată la fiecare unealtă nouă. |
| Un model trimite în buclă acțiuni externe | `EXTERNAL_LIMIT_PER_HOUR = 20` aprobate pe conector și pe oră. `external_gate` verifică limita înainte de întrebare; după aprobarea omului, `Store::try_record_external_approval` verifică și înscrie aprobarea într-un singur `INSERT ... SELECT` atomic. Două motoare/conexiuni nu pot ocupa simultan ultimul loc. Apelul care pierde cursa nu rulează și este auditat `blocked`. | Limita e pe conector și pe oră. Până la 20 de acțiuni pe oră rămân posibile, fiecare cu aprobare. Dacă două întrebări au fost deschise când mai era un singur loc, una poate fi blocată după răspunsul omului. |

**Întăriri E1, 2026-11-04:** exportul acceptă numai `.jsonl`/`.json`, backupul numai `.db`; prefixele UNC și extinse Windows sunt refuzate înaintea accesului la sistemul de fișiere. Fișierul temporar complet este publicat prin `hard_link`, care nu suprascrie o destinație apărută concurent. Aceste verificări nu transformă căile locale în sandbox pentru junction-uri sau procese ale aceluiași utilizator. Căutarea acceptă cel mult șase tipuri din lista fixă; filtrele invalide sunt erori, inclusiv la limita uneltei și RPC.

Conectorii refuză nume goale, ascund mesajul erorii primite din `check()` și încearcă refacerea secretelor anterioare dacă salvarea parțială sau scrierea SQLite eșuează. Salvările și eliminările sunt serializate în serviciu. Keychain-ul și SQLite nu formează o tranzacție comună: o oprire brutală sau un eșec al refacerii poate lăsa credențiale parțiale; eșecul refacerii este raportat explicit, fără valori secrete.

### 2.9 Conținut necunoscut

| Amenințare | Apărare existentă | Risc rămas |
|---|---|---|
| Textul din email, din pagini web sau din documente dă instrucțiuni modelului | În E2, snapshot-ul documentului și selecția sunt delimitate ca date în mesajul utilizatorului, nu în instrucțiunile de sistem. Uneltele workspace primesc o țintă/revizie și un nonce de tură; serviciul verifică contextul activ inclusiv în tranzacția care salvează propunerea. Modelul salvează propuneri; aplicarea este un RPC separat, declanșat de persoană. Modul, regulile și poarta External rămân aplicate. | Delimitarea nu garantează că modelul ignoră injecția. Un agent cu acces general la fișiere/shell sau un proces al aceluiași utilizator poate accesa baza și tokenul: API-ul de propuneri nu este un sandbox al sistemului de operare. Email/web rămân pentru planurile următoare. |

**Documente E2:** importurile sunt locale, UTF-8, mărginit citite și validate pentru `.txt`, `.md`, `.html`, `.csv`; exportul verifică revizia și publică prin `hard_link`, fără suprascriere. Prefixele UNC/extinse sunt refuzate, fără promisiune de izolare față de junction-uri locale. HTML este curățat în document inert înaintea editorului și afișat în iframe cu `sandbox` gol și CSP fără resurse externe, script, formulare sau navigare. Sursa originală se păstrează până la editarea/conversia explicită. CSV este date text; exportul sigur neutralizează formulele, exportul exact cere confirmare. Parserul și sanitizatorul au teste adversariale, fără certificarea absenței tuturor vulnerabilităților din dependențe.

**Scope E2:** registry-ul turei limitează și `search_workspace` la proiect/context activ, inclusiv în subagenți. `search_history` păstrează comportamentul global E1 pentru conversații/importuri. Izolarea operațiilor Item nu este o promisiune de confidențialitate între toate cunoștințele, uneltele, fișierele și procesele agentului.

**Reamintiri E2:** revendicarea generației și inboxul persistent se înscriu atomic înainte de apelul Windows. După o oprire cu rezultat incert, serviciul nu retrimite aceeași generație. Aceasta evită o nouă încercare duplicată, dar poate pierde toastul; inboxul rămâne. Un rezultat `submitted` nu dovedește afișarea sau citirea. Identitatea Windows este citită din shortcut-ul existent, niciodată creată în fundal; fără identitate/permisiune notificarea rămâne în inbox cu rezultat explicit. Schimbarea orei, ștergerea sau terminarea sarcinii anulează generația; restaurarea nu rearmează automat. Reamintirile cer serviciu pornit, nu pornesc un agent și nu trimit date în exterior.

**Instalator E2:** oprirea serviciului verifică PID-ul și executabilul deținut, trimite RPC autentificat `service.shutdown`, apoi așteaptă ieșirea. Un executabil ocupat refuză înlocuirea; hook-urile nu folosesc oprire după nume sau fallback de tip kill. Compilarea hook-urilor/pachetului nu dovedește instalarea, actualizarea ori dezinstalarea reală; probele fiecărui build sunt în raportul de validare E2.

## 3. Riscuri prioritare rămase

1. Modelul poate citi secrete dintr-un proiect și le poate trimite printr-o operație permisă/aprobată. Aprobările nu rezolvă prompt injection.
2. Shell-urile, MCP-urile, instalatorii și sarcinile evaluate rulează cu drepturile utilizatorului; Job Objects fac curățare, fără a oferi sandbox.
3. Un proces local al aceluiași utilizator poate citi tokenul serviciului, keychain-ul și istoricul necriptat. ACL-urile reale nu au fost auditate aici.
4. Fișierele GGUF fără hash, hash-urile furnizate de aceeași sursă și binarele găsite pe PATH au limite de autenticitate.
5. Modurile FullAccess, bypass și aprobările automate măresc impactul unei instrucțiuni malițioase.
6. Distribuția publică nu este configurată: instalatorul nu are Authenticode, endpointul updater este gol, iar instalarea unei actualizări semnate nu a fost verificată aici cap-coadă cu un endpoint și un instalator real.

## 4. Limitele analizei

Acest document nu certifică Windows Credential Manager, ACL-urile efective, llama.cpp, winget, Claude Code, Codex, dependențele Cargo/npm sau serverele furnizorilor. Nu am rulat instalări, comparații pe abonament ori apeluri cu cost pentru analiza documentului. Dovezile de testare și rezultatele reale sunt raportate separat în raportul de validare al planului D; ceea ce nu a fost rulat trebuie să rămână marcat ca atare.
