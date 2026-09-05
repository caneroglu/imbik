Baştan sona okudum — Rust'ın 6.5k satırı, `scripts/schematic.py`'nin 466 satırı, üretilmiş bench çıktıları ve checkpoint. Kanıtlarla gidiyorum.

## Önce şok edici olan: şema motoru devreni çiziyor sanıyorsun, çizmiyor

`bench/bjt_buffer/circuit.net` içinde 5 eleman var: `Q1, R1, R2, R3, C1`.
`bench/bjt_buffer/schematic.svg` içindeki bütün etiketler:

```
IN (Node 1) | OUT (Node 2) | C1 | 1uF
```

**Beşte dördü çizilmemiş.** Dosya 2122 byte — iki tel ve bir kondansatör. İki bağımsız sebep:

1. **BJT desteği hiç yok.** [schematic.py:93](scripts/schematic.py:93) → `twoterms = [c for c in components if c.get('comp_type') in ['R','C','D']]`. `Q` hiçbir listeye girmiyor, [get_element](scripts/schematic.py:35)'de dalı yok, `parse_netlist_file` de Q satırlarını atlıyor. `grep -i "Q\|bjt\|transistor"` → **sıfır sonuç**. Yani discrete BJT avına geçtiğin andan beri her şematiğinde transistör yok.
2. **Raya değen her iki uçlu eleman sessizce düşüyor.** [schematic.py:216](scripts/schematic.py:216) ve [:228](scripts/schematic.py:228) → `if u not in [3,4] and v not in [3,4]`. R1 (3→10), R2 (10→4), R3 (2→4) hiçbir kategoriye (backbone/shunt/bypass/bridge) giremiyor → çizilmiyor, uyarı da yok.

`circuit_44` düzgün görünüyor çünkü tesadüfen raya değen hiçbir R/C'si yok — op-amp topolojisi. Yani motor **sadece tek yollu op-amp/pasif devrelerde** çalışıyor, ki senin hedef domainin artık o değil.

Bu P0 çünkü çıktı **fiziksel kurulum belgesi**. Eksik şematikle breadboard kuruluyor.

## İkinci şok: sahte descriptor BOM'a kadar sızmış

[engine.rs:320](src/engine.rs:320):
```rust
let desc: BehaviorDescriptor = [best.fitness_score, 0.0, 0.0, ... ];
```
Preset modunda 11D descriptor hesaplanmıyor, yerine fitness gömülüyor. Zincir:

- `checkpoint_final.json` → 4 kaydın hepsi `[0.71, 0,0,0,...]`
- `describe_character`: `d[0]>0.5` → "Aktif Filtre", `d[1]=0` → `fc = 10^0 = 1 Hz`
- `evaluate_rarity`: `is_functional=false` → **her şey sonsuza kadar COMMON**
- ve `bench/bjt_buffer/bom.md` şunu yazıyor:

> **Topology Class**: Active Frequency Filter
> **Predicted Cutoff Frequency**: 1.0 Hz

Bu bir emitter follower. BOM da protokol de yalan söylüyor.

---

# Fix listesi

## P0 — Yanlış çıktıyı doğru gibi sunanlar

| # | Ne | Nerede | Fix |
|---|---|---|---|
| 1 | Şemada BJT yok | [schematic.py:35,72,93](scripts/schematic.py:93) | `elm.BjtNpn`/`BjtPnp` ekle, C yukarı / E aşağı / B sola |
| 2 | Raya değen elemanlar sessizce düşüyor | [schematic.py:216,228](scripts/schematic.py:216) | VCC/VEE yatay bus'ı çiz, raya değeni dikey drop yap |
| 3 | **Kapsam güvencesi yok** | schematic.py geneli | Sonda `assert placed == len(components)`; yerleşemeyeni "unplaced" bölgesine çiz + uyar. Sessiz düşürme asla |
| 4 | Preset modunda sahte descriptor | [engine.rs:320](src/engine.rs:320) | Arşive girecek adayda gerçek `extract_behavior_descriptor` koş |
| 5 | **`realism.rs`'te BJT modeli yok** | [realism.rs:72-73](src/realism.rs:72) | Sadece tl072+1N4148 include ediliyor → Monte Carlo her discrete devrede patlıyor → `mc_dev_db=None` → EPIC/LEGENDARY matematiksel olarak imkansız |
| 6 | `to_tran_netlist`'te de BJT modeli yok | [circuit.rs:304-305](src/circuit.rs:304) | Aynı |
| 7 | Netlist başlığı 4 yere kopyalanmış, 2'si eksik | circuit.rs ×2, realism.rs, fitness.rs | Tek `standard_spice_headers()`'a indir — 5 ve 6 kendiliğinden kapanır |
| 8 | OP okunamazsa devre gate'i **geçiyor** | [fitness.rs:247](src/fitness.rs:247) `unwrap_or(0.0)` | `ok_or(MissingNodeVoltage)` yap |

## P1 — Ölçüm matematiği (hesap kitap tarafı)

| # | Ne | Nerede | Detay |
|---|---|---|---|
| 9 | **AC referansı yanlış nokta** | [fitness.rs:408](src/fitness.rs:408) `ref_db = ac[0].mag_db` | Referans 10 Hz'deki kazanç. Ama her discrete devrende girişte kuplaj kondansatörü var → 10 Hz zaten sönüm bölgesinde. High-pass ve band-pass hiç tanınmıyor; `has_filter` pratikte "low-pass mi" demek. **Fix: referans = max\|H\|**, cutoff'u ondan -3dB ara |
| 10 | **`calc_asymmetry` asimetri değil DC offset ölçüyor** | [fitness.rs:568](src/fitness.rs:568) | `(v_pos − \|v_neg\|)/span`. Ortalama çıkarılmıyor. `asym_delta`'da span'lar farklı (0.2V vs 4V) olduğu için DC iptal de olmuyor: `asym_delta ≈ V_dc·(1/0.2 − 1/4)`. D4 boyutun fiilen DC offset haritası. **Fix: önce ortalamayı çıkar** |
| 11 | **Harmonik gürültü tabanı kalibre değil** | [fitness.rs:556](src/fitness.rs:556) eşik `1e-4` | `tran 10us` + `interpolate_tran` lineer interpolasyon 1 kHz'de ≈ (ωΔt)²/8 ≈ 5e-4 (−66 dB) sayısal distorsiyon üretiyor — eşiğin 5 katı. Mükemmel lineer devre "H3 var" diyor → sahte müzikal karakter → rarity gürültülü. **Fix: harmonik bench'inde `linearize` + 1us adım, sonra bilinen-lineer devreyle tabanı ölç ve eşiği ona göre koy** |
| 12 | Kazanç sıkışması giriş genliğini varsayıyor | [fitness.rs:604-612](src/fitness.rs:604) | `/0.2` ve `/4.0` hardcoded, netlistteki değerlerle ayrı yerde tutuluyor. Tran dosyasında **gerçek `v_in` kolonu zaten var** ama kullanılmıyor. Fix: gerçek v_in'den Vpp hesapla |
| 13 | Osilasyon frekansı sıfır-geçiş sayımıyla | [fitness.rs:645](src/fitness.rs:645) | Harmonik/gürültü varsa frekans şişer. D10 → "ultrasonik parazit" kararını, o da rarity'yi sürüyor. Fix: otokorelasyon veya FFT tepe |
| 14 | **Osilatörler çoğunlukla kaçırılıyor** | `tran_zero` bench'i | `alter @v_in[sin]=[0 0 0]` sonrası tran, yakınsamış DC OP'den başlıyor. SPICE kararsız dengede sonsuza kadar oturur — gerçek osilatör başlamaz. Fix: `.ic` ile küçük bozulma veya kısa darbe enjekte et |
| 15 | Probe frekansı ızgaraya oturmuyor | [fitness.rs:214](src/fitness.rs:214) `ac dec 10 100 100k` | En yakın nokta alınıyor; TOML'a 440 Hz yazarsan %12'ye kadar frekans sapması. 1 kHz tesadüfen ızgarada. Fix: log-lineer interpolasyon |
| 16 | `Zout` sabit `0.0` | [fitness.rs:348](src/fitness.rs:348) | TOML'a yazarsan sessizce sıfır skor alırsın. Fix: ya uygula (çıkışa test akımı) ya enum'dan çıkar |
| 17 | `condition.vin` ölü alan | grep: sadece `engine.rs:207`'de println | Spec vaat ediyor, motor kullanmıyor |
| 18 | `r_load` yalnız **ilk** probe'dan | [fitness.rs:186-192](src/fitness.rs:186) | Hepsine o uygulanıyor. Per-probe koşul fiilen yok |
| 19 | İki çelişen eşik seti | `ConstraintLimits::default` (dc 0.50 / margin 1.65) vs preset (`2.0` / `0.5`) | İki doğruluk kaynağı; novelty yolu ile preset yolu farklı fizik uyguluyor |
| 20 | Monte Carlo sadece R ve C'yi oynatıyor | [realism.rs:120](src/realism.rs:120) | Breadboard'daki en büyük sapma **BJT beta'sı** (2N3904 Bf 100–300!). Discrete devrelerde kırılganlığı sistematik olarak eksik ölçüyorsun |

## P2 — Arama / evrim tasarımı

| # | Ne | Nerede |
|---|---|---|
| 21 | `require` vs `objective` ayrımı yok | Ağırlıklı ortalama. Arşivdeki şampiyon: `DcOffset=1.64 (sc=0.00)` — probe tam sıfır, yine birinci |
| 22 | **Mutasyon rayları bağlayamıyor** | [mutate.rs:70](src/mutate.rs:70) `get_available_nodes` VCC/VEE'yi **çıkarıyor**. Ray bağlantısı yalnızca yok edilebiliyor, yaratılamıyor (tek istisna `add_component`'in hardcoded BJT dalı). Bias ağı gerektiren her topoloji için asimetrik, tek yönlü bozucu arama |
| 23 | Preset modunda arşiv jenerasyon başına 1 kayıt | [engine.rs:322](src/engine.rs:322). Loot havuzun = jenerasyon sayısı. 15 jenerasyon → 4 kayıt. Novelty seçimi hiç yok |
| 24 | Q model toggle topolojiyi bozuyor | [mutate.rs:113](src/mutate.rs:113) NPN↔PNP çevirirken bias ağı aynı kalıyor → gate hep eliyor → boşa eval bütçesi |
| 25 | Monte Carlo sıralı 21 ngspice | [realism.rs:154](src/realism.rs:154). Ayrıca preset yolunda **hiç çağrılmıyor** → mc her zaman None |
| 26 | Global eşzamanlılık limiti yok | Aday başına 2 ngspice process, rayon sınırsız fan-out |

## P3 — Altyapı / hijyen

| # | Ne |
|---|---|
| 27 | `ngspice` ve `uv` PATH'ten sert çağrılıyor ([spice.rs:110](src/spice.rs:110), [loot.rs:483](src/loot.rs:483)); bu makinede ngspice PATH'te **değil**. Config'e yol + anlamlı hata |
| 28 | Hata tespiti `log.contains("Error:")` — singular matrix / yakınsamama yakalanmıyor, ngspice 0 dönüyor |
| 29 | `Box::leak` ([loot.rs:209](src/loot.rs:209)) her `list`/`show` çağrısında tüm arşivi sızdırıyor. Owned struct döndür |
| 30 | **`imbik ingest` komutu yok** ama protokol ([bench.rs:342](src/bench.rs:342)) onu çalıştırmanı söylüyor. LEGENDARY tanımın "donanımda doğrulandı" ama `evaluate_rarity` donanım girdisi almıyor → tier uygulanamaz durumda |
| 31 | Protokol metni op-amp merkezli sabit: saf discrete devrede "TL072 çipinin yönünü kontrol et", "op-amp raya doymuş" diyor |
| 32 | `reference.csv`'de `v_in` **analitik üretiliyor** ([bench.rs:370](src/bench.rs:370)), simülasyonun gerçek v_in'i kullanılmıyor — scope karşılaştırmasının referansı bu |
| 33 | `.gitignore` sadece `/target`. `scratch/` (45 SVG + `__pycache__`), `checkpoints/`, kökteki 2 stray SVG hepsi takipte |

---

## Sıra (bunu böyle yaparım)

**Blok 1 — yalanları durdur (1 gün).** 7 → 5,6 kendiliğinden kapanır → 8 → 4. Sonra 1,2,3. Bittiğinde: şematik gerçek devreyi gösteriyor, BOM doğru sınıf yazıyor, Monte Carlo çalışıyor, loot tier'ları fiilen ateşleniyor. Kabul testi: `bjt_buffer` şematiğinde 5 etiketin 5'i de görünecek.

**Blok 2 — ölçüm doğruluğu (1-2 gün).** 9, 10, 12 (üçü de tek satırlık matematik düzeltmesi, en yüksek getiri). Sonra 11'i kalibre et, 14'ü ekle. Bu blok bitmeden descriptor'a dayalı hiçbir şeye (novelty, rarity, karakter) güvenme.

**Blok 3 — spec omurgası (2-3 gün).** 21 → 18/17/16 → bench gruplaması + `Analysis` enum + metrik kataloğu. Geçen mesajdaki TOML şekli burada devreye girer.

**Blok 4 — arama kalitesi (1 gün).** 22 (rayları node havuzuna al, tek satır, arama uzayını gerçekten açar), 23, 24, 25.

**Blok 5 — hijyen.** 27–33.

**Not:** 22 tek satırlık bir değişiklik ama etkisi 3. blok kadar büyük — mutasyon operatörlerin şu an bias ağı *kuramıyor*, sadece bozabiliyor. Blok 1'e ekleyip erken görmek istersen mantıklı olur.

`cargo check --all-targets` temiz geçiyor, yani bunların hiçbiri derleme hatası değil — hepsi sessiz davranış hatası. En tehlikeli tür.