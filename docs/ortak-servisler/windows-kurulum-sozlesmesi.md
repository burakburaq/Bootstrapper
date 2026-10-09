# Windows Ortak Kurulum Sözleşmesi — v1

Son güncelleme: **8 Ekim 2026**. Durum: **ortak update-client/Windows servis kodu uygulandı; ürün entegrasyonu ve gerçek Windows kabulü açık**.

Bu sözleşme installer, uninstaller, uygulama ve offline güncelleme servisinin ortak sınırıdır. Baykuş Lig politikası [ürün belgesinde](../baykuslig/urun/guncellemeler-ve-platform-destegi.md); kabul edilen kararlar [DEC-C056](../baykuslig/kararlar/kapatilan-kararlar-c056.md) ve [DEC-C058](../baykuslig/kararlar/kapatilan-kararlar-c058.md); servisin paket doğrulama kuralları [offline servis belgesindedir](./windows-offline-kurulum-servisi.md).

[DEC-C060](../baykuslig/kararlar/kapatilan-kararlar-c060.md) küçük installer, birleşik app/açılış ekranı, onarım, kalıcı indirme devamı ve iki kaldırma kutusunu kaydeder. Platformlar arası sınırlar [ortak masaüstü yaşam döngüsündedir](./masaustu-kurulum-yasam-dongusu.md). Buradaki native installer bootstrapper'ı uygulama içindeki açılış ekranından ayrı bir süreçtir; ayrı kalıcı uygulama launcher'ı kurulmaz.

Mevcut 0.1.0 Tauri/NSIS çıktısı kullanıcı başına kurulur; yeni perMachine düzeni bu eski binary'ye uygulanmış sayılmaz. 8 Ekim tesliminde `updates/` içinde Windows adaptörü, offline LocalSystem servis/IPC, envanterli NSIS üretimi, ortak journal ve imzalı kurulum izni uygulandı. App/installer bağlaması ve gerçek imzalı ürün dağıtımı tamamlanmadı. [Kod doğrulaması ve açık kabul](../planlama/dogrulama/update-client-installer.md), [native entegrasyon sırası](../../updates/crates/update-client/README.md).

## Bileşenler ve yetki

| Bileşen | Sorumluluk |
|---|---|
| Native installer bootstrapper | Ortam ve WebView2 kontrolü; yerel web ekranı veya native uyarı/fallback; online paket hazırlama ve kullanıcı onayı |
| HTML/CSS/JS arayüz | Seçimleri ve native durumunu gösterme; dar komut köprüsü üzerinden talep gönderme |
| Yetkili NSIS worker | İlk kurulum, update, onarım/kaldırma; Program Files, program kaydı ve SCM servis kaydı |
| Offline Windows servisi | Normal güncellemede bağımsız doğrulama, korumalı staging, journal ve kurulum |
| Uygulama / update-client | Çevrimiçi sürüm/lifecycle kontrolü, indirme ve delta hazırlama, güncelleme onayı, güvenli çıkış ve normal kullanıcı olarak yeniden açılma |

İlk kurulum ve kaldırma için UAC gerekir. Web ekranı mümkün olduğunca normal kullanıcı yetkisiyle çalışır; dosya/servis değişiklikleri ayrı yetkili worker'dadır. Sonraki normal güncellemeyi önceden kurulmuş servis yapar; tekrar UAC istemez. Eksik/bozuk servis için gereken onarım ayrı ve açık bir yönetici işlemidir.

UI'nin onay, sürüm veya “doğrulandı” bilgisi yetki kanıtı değildir. Yetkili taraf ürün kaydını, çağıranı, işlemi ve paketi kendisi kontrol eder. Servis genel dosya silme, shell çalıştırma veya başka ürün kurma API'si sunmaz.

Installer uygulama payload'unu içermez; uyumlu izinli sürümü normal kullanıcı olarak indirir/ön doğrular, ayrı yetkili worker tam imzalı NSIS paketiyle app + servisi kurar. İlk kurulum internet gerektirir. Uyumsuz eski bootstrapper yeni resmî installer'a yönlendirilir. Servis güncellemesinde NSIS worker sessiz çalışır ve servisin durmasından etkilenmeden self-update'i tamamlayabilir; bu entegrasyon henüz uygulanmadı. App ve kendi açılış ekranı aynı binary olduğundan dosya değişiminden önce kapanır; yeniden açılma ilk kullanıcının normal oturumundadır.

## Ürün ve kurulum kimliği

| Alan | Anlam / Baykuş Lig eşlemesi |
|---|---|
| `install_contract_version` | Bu yerel sözleşmenin sürümü: `1`; API protokolü veya update-protocol schema sürümü değildir |
| `app_id` | Update ekosistemindeki ürün: `baykuslig` |
| `bundle_id` | Masaüstü uygulama kimliği: `com.talaylabs.baykuslig` |
| `installation_id` | İlk makine kurulumunda üretilen rastgele UUID; ortak korumalı kayıt |
| `scope` | Yalnız `per_machine`; UI kapsam seçtirmez |
| `architecture` / `target` | Paketin gerçek mimarisi ve yayın hedefi; birlikte doğrulanır |
| `channel` | İmzalı dağıtım/ürün kaydından gelir; UI serbest kanal değiştiremez |
| `installed_version` | Kurulan üründen doğrulanan SemVer; UI veya installer çıkış kodundan alınmaz |
| `release_id` / `sequence` | İmzalı update metadata'sındaki yayın ve anti-rollback sırası; SemVer yerine geçmez |

Normal update ve onarım `installation_id` değerini korur. Tam kaldırma başarıyla tamamlandığında eski kimlik emekli edilir; sonraki ilk kurulum yeni rastgele UUID üretir ([DEC-C059](../baykuslig/kararlar/kapatilan-kararlar-c059.md)). Kullanıcı verisini koruma seçimi bu kurulum kimliğini korumaz. Windows kullanıcısı, uygulama hesabı veya cihaz donanımı bu kimliği belirlemez. Yarım/başarısız kaldırmada önce mevcut işlem uzlaştırılır; yeni kimlikle devam edilmez. Ortak journal'da emekli kimlik işareti, sonraki bootstrap'ta atomik UUID yenileme ve servis bağlamını yenileme kodu uygulandı; hedefli testleri geçti. Gerçek Windows kaldırma/yeniden kurma kabulü açıktır.

Paket/ürün hedefi uyuşmazlığı reddedilir. Update config örneğinde `windows-x86_64-nsis` bulunur; ARM64 hedefinin kesin adı ve kayıt eşlemesi update sahibiyle tamamlanacaktır. Kanal geçişi normal update olarak örtük yapılmaz.

## Dosya alanları

Yollar Windows known-folder API'leriyle çözülür; `C:` veya kullanıcı adı sabitlenmez. UI keyfi kurulum yolu göndermez. Baykuş Lig için v1 hedef düzeni:

| Alan | Yol | Kural |
|---|---|---|
| Program | `<ProgramFiles>/TalayLabs/BaykusLig/` | Mimariye uygun program alanı; app, servis ve native kaldırıcı; normal kullanıcı yazamaz |
| Ortak durum | `<ProgramData>/TalayLabs/BaykusLig/` | Kurulum kimliği, ürün kaydı ve korumalı servis durumu |
| Staging | Ortak durum altında `staging/` | Servisin kontrol ettiği paket kopyası; uygulamanın indirme dizini yetkili staging sayılmaz |
| Journal | Ortak durum altında `journal/` | İşlem/sürüm/sequence ve kesinti sonrası uzlaştırma kaydı |
| Kullanıcı verisi | `<LocalAppData>/com.talaylabs.baykuslig/` | İlgili Windows kullanıcısının ayar ve WebView profili; normal update korur |
| Kimlik sırları | İlgili kullanıcının Windows Credential Manager alanı | Dosya cache'inden ayrıdır; installer paketine/journal'a/log'a kopyalanmaz |

Kayıt, hedef yolları ve ürün sahipliğini doğrulanabilir biçimde tutar; kullanıcı tarafından değiştirilebilen bir JSON veya registry değeri tek başına silme/çalıştırma yetkisi vermez. ProgramData ACL'leri uygulama sırasında offline servis sınırlarına göre kurulacaktır. Kullanıcı verisi veya indirme cache'i servis tarafından executable kaynağı olarak çalıştırılmaz.

## Installer ve uninstaller ekranı

Baykuş Lig installer ve uninstaller web pencereleri **1120×770 mantıksal piksel** olarak sabittir. Minimum ve maksimum iç boyut aynıdır; kullanıcı yeniden boyutlandıramaz veya maximize edemez. Taşıma, küçültme ve kapatma kullanılabilir. Windows DPI ölçeklendirmesi fiziksel piksel sayısını değiştirebilir. Native runtime uyarısı/kaldırma fallback'i sistem dialog düzenini kullanır.

1. Native installer bootstrapper desteklenen platform/mimariyi ve WebView2'nin gerçekten başlatılabildiğini kontrol eder.
2. WebView2 kullanılabiliyorsa paket içindeki HTML/CSS/JS ekranı açılır. Ekranı göstermek için ağ erişimi gerekmez.
3. İlk installer'da WebView2 yoksa veya başlatılamıyorsa native uyarı çıkar: **resmî indirme sayfasını aç**, **yeniden dene**, **kapat**. Otomatik runtime indirme/kurma yoktur; runtime setup içine gömülmez.
4. Uninstaller'da aynı durumda **native kaldırıcı** çalışır. Kaldırma işlemi WebView2 yüklenmesini gerektirmez.

Resmî indirme bağlantısını açmak yalnız kullanıcının seçtiği normal yetkili tarayıcı eylemidir. Yetkili worker, servis ve paket doğrulaması ağdan bağımlılık çekmez. Servisin sessiz NSIS update yolu özel web ekranını açmaz.

Web ekranı yalnız paketle gelen kaynakları yükler. Uzak sayfa, genel shell köprüsü veya keyfi dosya çalıştırma yoktur. WebView2 indirme bağlantısı native tarafta sabittir. Önkontrol prototipindeki ilk görselin launcher tasarımı olduğu kullanıcı tarafından düzeltildi; installer görseli geçicidir. Uninstaller tasarımı referans olarak projededir, kaldırma uygulaması henüz yoktur.

## Yerel UI köprüsü

Bu bölüm v1 tasarım arayüzüdür; mevcut Tauri komutlarının veya update-client trait'inin teslim edildiği anlamına gelmez. İstekler sözleşme sürümü ve `request_id` taşır; mutasyonlar ayrıca `operation_id` taşır.

Bootstrapper, worker ve servis destekledikleri sözleşme sürümlerini bildirir. Ortak sürüm bulunamazsa mutasyon başlamaz; `contract_incompatible` ile onarım/yükseltme yönlendirmesi gösterilir. Bilinmeyen alan veya komut yeni bir çalıştırma yetkisine dönüştürülemez. UI sürümü, kurulu uygulama SemVer'i ve sözleşme sürümü ayrı değerlendirilir.

| Komut | İzin verilen iş |
|---|---|
| `inspect` | Platform, WebView2, mevcut ürün/scope ve bekleyen işlem özeti |
| `preview` | Sabit ürün için kurulum/onarım/kaldırma etkileri; iki temizleme seçiminin kapsamı |
| `start` | Önizlenen ve açıkça onaylanan `install`, `repair` veya `uninstall` işlemi |
| `status` | İşlem kimliğiyle authoritative durum/sonuç alma |
| `cancel` | Henüz mutasyon başlamadıysa iptal isteği |
| `launch` | Başarılı kurulumdan sonra kayıtlı uygulamayı ilk kullanıcının normal oturumunda açma |
| `open_webview_download` | Sabit resmî WebView2 sayfasını açma |

Ürün, channel, binary yolu, NSIS argümanları, servis adı veya silinecek serbest yol UI girdisi değildir. Güncelleme için ayrı uygulama–servis handoff'u gerekir; bu köprüye `update` seçeneği ekleyerek doğrulama/onay sınırı atlanamaz.

Aynı `operation_id` ve aynı niyet tekrar geldiğinde işlem yeniden uygulanmaz; mevcut durum/sonuç döner. Aynı kimlikle farklı niyet reddedilir. İlk kurulum, onarım, kaldırma ve update aynı ürün kilidini paylaşır; eşzamanlı dosya/servis değişikliği yapılamaz. UI kapanırsa worker öldürülmez; tekrar açılınca `status` ile devam eden işlem görülür.

## Durum, başarı ve kesinti

Native taraf aşamayı bildirir: `checking`, `awaiting_consent`, `awaiting_elevation`, `waiting_for_exit`, `verifying`, `applying`, `reconciling`; sonuç `completed`, `cancelled`, `deferred`, `failed` veya `recovery_required` olabilir. Bunlar UI durumlarıdır; update-client'ın mevcut `InstallPhase` enum'una örtük eşlenmez.

Normal kullanıcı indirme durumları `downloading` ve `paused` olarak ayrılır; yarım parçayı saklayıp devam etme/iptal edip silme kuralları [ortak yaşam döngüsündedir](./masaustu-kurulum-yasam-dongusu.md#yarım-indirme-ve-onarım). Bunlar henüz native komut/transport uygulaması değildir. Onarım aynı izinli tam sürümü tekrar uygulayabilir; normal update'in anti-rollback sınırını kaldırmaz, kullanıcı verisi/installation_id'yi korur. App/servis onaramıyorsa resmî installer ve gerekirse UAC kullanılır.

Durum yanıtı en az sözleşme sürümü, `operation_id`, işlem türü, aşama, iptal edilebilirlik ve varsa hata kodu taşır. Yüzde ancak native taraf ölçebiliyorsa verilir; aksi halde belirsiz ilerleme gösterilir. Hata yanıtında anlaşılır mesaj ve destek için istek/işlem kimliği bulunur; parola, MFA, token veya private key bulunmaz.

- Mutasyon öncesi kullanıcı iptali ve UAC reddi temiz şekilde sona erer. `applying` aşamasında iptal düğmesi worker'ı zorla sonlandırmaz.
- Programı kullanan bütün Windows oturumları dikkate alınır. Güvenli kapanış sağlanmazsa işlem ertelenir; kullanıcı verisi kaybettiren zorla kapatma yapılmaz.
- NSIS çıkış kodu tek başına başarı değildir. Gerçek kurulu sürüm/dosyalar, beklenen servis ve program kaydı ile journal sonucu doğrulanır. Kaldırmada kayıtlı program ve servisin kaldırıldığı doğrulanır; kendi çalışan EXE'sinin temizliği native kaldırıcı tarafından tamamlanır.
- Kesinti sonrası önce korumalı journal ve gerçek kurulu durum uzlaştırılır. “Başarılı” gösterip yeni işlem başlatılmaz; gerekirse `recovery_required` döner. Otomatik sürüm düşürme yapılmaz.
- Servis paketi, yayın imzası, hash/boyut, Authenticode yayıncısı, hedef ve sequence/checkpoint kurallarını bağımsız doğrular. Son lifecycle izni için ayrı operasyonel server anahtarıyla nonce/işlem/kimlik/niyet/release'e bağlı imzalı izin ve monoton süre sınırı uygulandı; app bağlantısı/gerçek dağıtım kabulü açıktır. UI boolean'ı veya eski imzalı manifest tek başına yeterli değildir.

## Kaldırma ve eski kurulum

Kaldırma programı, kayıtlı offline servisi, ürünün program kaydını ve kısayollarını temizler. Paylaşılan WebView2 Runtime kaldırılmaz. Sunucudaki hesap/veri silinmez; offline kaldırma uzaktaki oturumların iptal edildiği iddiasında bulunmaz.

Kullanıcı verisi ve ayarlar **varsayılan olarak korunur**. Varsayılan boş iki bağımsız kutu vardır: **Kullanıcı verilerini sil** (hesap/profil/oturum, ilgili credential namespace'i ve cache) ve **Yerel ayarları kaldır** (dil/tema/bildirim/hızlı giriş). Seçimler ayrı boolean alanlarıdır; serbest yol girdisi değildir. Veri ve ayar alanları bağımsız temizlenebilecek biçimde ayrılır; veri silinse de kalan hızlı giriş tercihi eski hesabın oturumunu geri getiremez.

Temizleme yalnız işlemi başlatan Windows kullanıcısının ürün verisine uygulanır; başka kullanıcıların profilleri taranıp silinmez. UAC başka yönetici hesabıyla onaylandıysa ilk kullanıcı SID'si korunur; yöneticinin LocalAppData'sı yanlışlıkla hedeflenmez. Credential Manager temizliği ilk kullanıcının yetkisinde ve yalnız uygulamanın kimlik namespace'inde yapılır; klasör silmenin sırları da sildiği varsayılmaz. EBWebView profilinin normal çalışma davranışı değişmez.

Mevcut kullanıcı başına 0.1.0 kurulum tespit edilirse `migration_required` döner. Program Files kurulumuna sessizce terfi ettirilmez, eski kullanıcı dizinindeki binary veya ayarlar yetkili kurulum kaynağı sayılmaz. Geçiş akışı ayrı olarak uygulanacak; mevcut hesap/cache taşıma ve eski uninstall işlemleri açıkça gösterilecektir.

## Uygulama öncesi kalan kararlar

| Konu | Tamamlanacağı aşama |
|---|---|
| Eski currentUser kurulumu için geçiş akışı | Installer temeli; mevcut DEVTEST kurulumunu değiştirmeden önce |
| Ortak journal/staging ve ürün kaydının kesintiye dayanıklı temizliği | Uninstaller uygulaması; kimlik ömrü DEC-C059 ile kapandı, kalan temizlik/uzlaştırma ayrıntıları uygulanacak |
| Servis hesabı/ACL, SCM/IPC adları ve sürüm uyumluluğu | Update sahibiyle servis entegrasyonu |
| Son kurulum izninin app bağlantısı/gerçek kabulü; kesintiye dayanıklı servis self-update | İzin mekanizması ortak kodda uygulandı; gerçek update etkinleştirilmeden önce kabul. Servis binary/politika değişimi şimdilik ayrı bakım dağıtımı ister |
| ARM64 target kaydı, üretim yayıncısı/public trust roots ve kanal eşlemesi | İmzalı dağıtım entegrasyonu |

Bu açık noktalar belgeyi veya installer bootstrapper iskeletini hazırlamayı engellemez; ilgili gerçek mutasyon davranışları netleşmeden tamamlanmış ilan edilemez.

## Kabul sırası

1. Installer temeli: mevcut/eski kurulum önkontrolü, perMachine yolları, UAC, WebView2 var/yok/başlatılamıyor akışları.
2. Uninstaller: web ekranı ve native fallback, uygulama açıkken erteleme, iki bağımsız veri/ayar seçiminin dört birleşimi ve yalnız ilk kullanıcıyı temizleme.
3. Update entegrasyonu: offline bağımsız doğrulama, ortak kilit/journal, aynı işlemi tekrar gönderme, güç/worker kesintisi, servis onarımı ve self-update.
4. Kullanıcının HTML/CSS tasarımıyla Windows 10/11 x64 ve Windows ARM64 gerçek kabulü; geniş platform desteği yalnız bu kontrollerden sonra tamamlanır.

[Installer önkontrol/UI temeli](../../apps/installer/README.md) ayrı kaynak olarak eklendi. Windows sürümü/mimari/known-folder yolları, registry kurulum ipuçları ve gerçek WebView2 başlangıcı uygulanmıştır. Kalkan tıklamasında ayrı UI'sız yönetici yardımcısını açan UAC akışı eklenmiştir; normal UI `asInvoker` kalır. Son kaynak 7 Ekim 09:20'de Windows x64 release olarak derlendi; canlı UAC kabulü açık. Henüz indirme, NSIS, servis, kaldırma veya update komutu yoktur. [Yerel doğrulama kaydı](../planlama/dogrulama/windows-installer-temeli.md) çalıştırılan kontrolleri ve açık kabul işlerini ayırır; bu temel bütün kurulum sözleşmesinin teslimi değildir.
