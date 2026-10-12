# Scratchpad

Pieni, reunaton ja kelluva muistilappu. Ikkunassa on vain välttämätön: useita muistioita välilehdillä, muistion piilotus katseilta (API-avaimet ym.), kopiointi leikepöydälle, tallennus tiedostoon ja "kellu kaiken päällä" -nasta. Kaikki tallentuu itsestään.

![Scratchpad](docs/screenshot.png)

```
cargo run --release
```

| Toiminto | Hiiri | Näppäin |
|---|---|---|
| Uusi muistio | `+` tai tuplaklikkaus välilehtirivin tyhjään kohtaan | Ctrl+N / Ctrl+T |
| Sulje muistio (Undo toastista) | välilehden `×` tai hiiren keskinappi | Ctrl+W |
| Nimeä uudelleen | tuplaklikkaus välilehteen tai `…` → Rename | |
| Vaihda muistiota | klikkaus | Ctrl+Tab / Ctrl+Shift+Tab |
| Piilota/näytä sisältö | silmä | Ctrl+Shift+H |
| Kopioi kaikki | kopioi-ikoni | Ctrl+Shift+C |
| Tallenna tiedostoon | lataa-ikoni | Ctrl+S |
| Tyhjennä (Undo toastista) | roskakori | |
| Fontti (Inter / JetBrains Mono, koko) | `Aa` | |
| Teema (tumma/vaalea) | `…` | |
| Kellu päällä | nasta | |
| Siirrä ikkunaa / muuta kokoa | raahaa otsikkoriviltä / reunoista | |

Muistiot tallentuvat tiedostoon `~/.config/scratchpad/notes.json` (macOS: `~/Library/Application Support/scratchpad/`, Windows: `%APPDATA%\scratchpad\`). Piilotus vain peittää sisällön näytöltä, joten tiedosto on edelleen tavallista tekstiä.

## Miten ja miksi Rust

**Miksi.** Muistilapun pitää käynnistyä heti ja olla kevyt, eikä se saa kaatua. Rust kääntyy yhdeksi natiiviksi binääriksi ilman ajonaikaista ympäristöä, roskienkerääjää tai selainmoottoria. Electron-versiossa olisi mukana koko Chromium. Sama koodi kääntyy Linuxille, macOS:lle, Windowsille ja tarvittaessa WASM:ksi. Tyyppijärjestelmä ja omistajuus estävät jo käännösvaiheessa tyypillisiä virheitä, kuten jaetun tilan kilpailutilanteet tallennussäikeen ja UI:n välillä.

**Miten.** Sovellus käyttää samaa mallia kuin pdfcraft. Sovellus omistaa jokaisen pikselin, ja käyttöjärjestelmältä pyydetään vain ikkuna:

1. **egui piirtää kaiken.** egui on *immediate mode* -GUI: `App::draw` kuvaa koko käyttöliittymän joka ruudunpäivityksellä. eframe ja wgpu vievät kolmiot GPU:lle (Metal, DX12/Vulkan, Vulkan/GL), ja winit antaa pelkän ikkunan ja syötteen. Ikkuna on luotu asetuksilla `with_decorations(false)` ja `with_transparent(true)`. Pyöristetyt kulmat, varjot ja painikkeet piirretään itse.
2. **Oma design-järjestelmä.** `src/theme.rs`: jokainen väri ja säde on `Tokens`-rakenteessa (tumma ja vaalea teema). Fontit (Inter, JetBrains Mono, OFL) upotetaan binääriin. `src/icons.rs`: Lucide-SVG:t (ISC) rasteroidaan juuri oikeaan kokoon ja sävytetään teeman värillä. Unicode-merkkejä ei käytetä ikoneina.
3. **Alustaerot hoidetaan reunoilla.** Leikepöytä hoidetaan eframen kautta (`ctx.copy_text`) ja natiivi tallennusdialogi rfd:llä omassa säikeessään, jotta ikkuna piirtyy dialogin ollessa auki. Ikkunan siirto ja koon muutos tehdään `ViewportCommand::StartDrag`- ja `BeginResize`-komennoilla, koska reunatonta ikkunaa ei voi raahata järjestelmän otsikkopalkista.
4. **Lopputulos tarkistetaan katsomalla.** `tests/ui.rs` ajaa oikeaa käyttöliittymää egui_kittestillä ilman näyttöä. Testi klikkaa painikkeita accesskit-nimien perusteella ja renderöi kuvat `shots/`-hakemistoon wgpu:lla. Samat accesskit-nimet välittyvät ruudunlukijoille.

Kääntöpuoli: sovellus ei näytä natiivilta. Lisäksi binääri on wgpu:n takia noin 15 Mt. Pienempi on mahdollinen eframen `glow`-taustalla.

## Wayland / Hyprland

Waylandissa sovellus ei voi itse päättää, onko ikkuna "aina päällä", eikä se voi määrätä ikkunan sijaintia. Siitä päättää kompositori. Nasta toimii X11:ssä, macOS:ssä ja Windowsissa winitin `WindowLevel`-asetuksella.

**Hyprland** tunnistetaan `HYPRLAND_INSTANCE_SIGNATURE`-muuttujasta, ja sen IPC-socketin kautta sovellus tekee saman kuin Omarchyn SUPER+O:
- Nasta tekee ikkunasta kelluvan ja kiinnitetyn. Kun nasta irrotetaan, kiinnitys poistuu, mutta ikkuna jää kellumaan.
- Kun ikkuna kiinnitetään tai irrotetaan Hyprlandin omilla näppäimillä (SUPER+O tai SUPER+T), nastan tila päivittyy noin sekunnin kuluessa.
- Kelluvan ikkunan koko ja sijainti muistetaan, ja ne palautetaan käynnistyksessä. Tiilitetyn ikkunan kokoa ei tallenneta.
- Komennot lähetetään ensin uudella Lua-syntaksilla (`hl.dsp.window.pin({ action = "enable", … })`, Hyprland 0.55+). Jos se ei onnistu, käytetään vanhaa `pin`/`setfloating`-syntaksia.

Muualla koko ja, missä alusta sen kertoo, myös sijainti luetaan winitiltä.

## Testit

```
cargo test            # yksikkötestit + headless UI-testit, kuvat shots/-hakemistoon
```
