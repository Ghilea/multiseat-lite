# Multiseat Lite

Ett första React/Tauri-skal för att administrera en multiseat-miljö: anslutna enheter, skärmar, platser och virtualisering.

Milestone 2 innehåller två logiska platser (`Dennis` och `Barnen`) med validerad tilldelning av fysisk display, keyboard och mouse. Ändringar sparas automatiskt i användarens appkonfigurationskatalog, inte i repositoryt. På Windows är standardplatsen:

```text
%APPDATA%\se.multiseat.lite\config.json
```

Formatet är versionsmärkt (`version: 1`). En giltig tidigare fil säkerhetskopieras som `config.json.bak` vid nästa skrivning. Frånkopplade tilldelningar behålls och visas som saknade.

Milestone 2 har även ett interaktivt identifieringsläge för tangentbord och möss. Knappen
`Identify keyboard/mouse` startar en tillfällig Raw Input-lyssnare och markerar det
korrelerade fysiska PnP-kortet när användaren flyttar en mus eller trycker på en tangent.
Korrelationen använder stabilt PnP-ID och Container ID, aldrig enbart enhetens visningsnamn.
Lyssnaren stoppas när läget stängs av, fönstret stängs eller programmet avslutas.

När ASTERs inputkomponenter faktiskt körs eller är synliga representerar Raw Input endast
det som är synligt i den aktuella workplace/sessionen. MutEnx-enheter behålls i diagnostiken
men behandlas som ASTER-proxyer, inte som tilldelningsbar fysisk hårdvara. Det innebär att
en fysiskt ansluten enhet kan finnas i PnP-inventeringen utan att kunna identifieras
interaktivt från aktuell workplace.

Keyboard- och mouse-devnodes aggregeras till en tilldelningsbar fysisk enhet per Windows
Container ID. Composite-enheter kan ha både keyboard- och mouse-capability och reserveras
alltid som en hel fysisk enhet för samma seat. Om Container ID saknas används det stabila
PnP instance-ID:t som fallback; enheter slås aldrig ihop utifrån namn eller VID/PID.
`hardware_probe` och `input_probe` behåller de logiska medlemsnoderna och Raw Input-vägarna
för felsökning. Äldre sparade devnode-ID:n migreras automatiskt när de entydigt kan kopplas
till ett fysiskt aggregat.

## Förutsättningar

- Node.js 20 eller senare
- Rust (stable)
- Tauri-prerequisites för operativsystemet

På Windows krävs Microsoft C++ Build Tools och WebView2. Se [Tauris prerequisites](https://v2.tauri.app/start/prerequisites/) för detaljer.

## Kom igång

```sh
npm install
npm run tauri dev
```

Frontend kan även köras fristående i webbläsaren:

```sh
npm run dev
```

## Kontroller

```sh
npm run check
cargo check --manifest-path src-tauri/Cargo.toml
```

Rust-koden är indelad i `devices`, `displays`, `seats` och `virtualization`. På Windows använder den första hårdvarumilstolpen SetupAPI för närvarande anslutna tangentbords- och musnoder, Raw Input för korrelation mot inmatningsvägar samt Win32 display-enumerering för aktiva bildskärmar. Windows Container ID visar vilka logiska HID-kollektioner som hör till samma fysiska enhet.

Kör diagnostiken utan UI för att se de identifierare som Windows rapporterar:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --example hardware_probe
```

`hardware_probe` redovisar GDI-skärmar för den aktuella sessionen, närvarande PnP-monitorer och aktiva/inaktiva DisplayConfig-mål separat. ASTER redovisas som separata installations-, register-, PnP- och tjänstesignaler; tjänsternas runtime-status läses skrivskyddat via Windows Service Control Manager. Workplace-läget rapporteras som `unknown` eftersom ingen dokumenterad skrivskyddad kontroll som används här kan fastställa det säkert. Raw Input-vyn är sessions-/workplace-specifik och ska inte tolkas som en fullständig fysisk inventering när ASTERs inputkomponenter körs.

En fokuserad inputdiagnostik finns också:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --example input_probe
```

## Milestone 3A — skrivskyddad backend-probe

Backendlagret definierar en virtualiseringsoberoende `SeatBackend` och typed capability-,
validation- och runtime-modeller. Aktivering är avsiktligt avstängd i denna milstolpe;
`start` och `stop` gör inga systemändringar.

Den skrivskyddade proben redovisar Windows-version/build, processor- och hypervisorflaggor,
Hyper-V-feature/tjänster, VirtualBox/VBoxManage och ASTER-runtimeinformation. Om VirtualBox
finns parsas `VBoxManage list usbhost`, utan att några USB-enheter ansluts eller kopplas bort.
Fysiska inputcontainrar matchas konservativt mot VirtualBox USB: unikt serienummer ger
`exact`, ett ensamt VID/PID-alternativ ger `unambiguous`, och identiska oskiljbara kandidater
ger `ambiguous` i stället för ett godtyckligt val.

```sh
cargo run --manifest-path src-tauri/Cargo.toml --example backend_probe
```
