"""Compact instance labels inferred from the parser's canonical encounter names."""
RAIDS = (
    ('AQ40 — Храм Ан’Киража', {
        'The Prophet Skeram', 'Battleguard Sartura', 'Fankriss the Unyielding',
        'Princess Huhuran', 'Twin Emperors', "C'Thun", 'Bug Trio', 'Viscidus', 'Ouro',
        "Emperor Vek'lor", "Emperor Vek'nilash", 'Princess Yauj', 'Vem', 'Lord Kri'}),
    ('AQ20 — Руины Ан’Киража', {'Kurinnaxx', 'General Rajaxx', 'Moam',
        'Buru the Gorger', 'Ayamiss the Hunter', 'Ossirian the Unscarred'}),
    ('ЦЛК — Цитадель Ледяной Короны', {'Lord Marrowgar', 'Lady Deathwhisper',
        'Gunship', 'Gunship Battle', 'Deathbringer Saurfang', 'Festergut', 'Rotface',
        'Professor Putricide', 'Blood Prince Council', "Blood-Queen Lana'thel",
        'Valithria Dreamwalker', 'Sindragosa', 'The Lich King'}),
    ('ИК — Испытание крестоносца', {'Northrend Beasts', 'Lord Jaraxxus',
        'Faction Champions', "Twin Val'kyr", "Anub'arak"}),
    ('Ульдуар', {'Flame Leviathan', 'Ignis the Furnace Master', 'Razorscale',
        'XT-002 Deconstructor', 'Assembly of Iron', 'Kologarn', 'Auriaya',
        'Hodir', 'Thorim', 'Freya', 'Mimiron', 'General Vezax', 'Yogg-Saron', 'Algalon the Observer'}),
    ('Наксрамас', {"Anub'Rekhan", 'Grand Widow Faerlina', 'Maexxna',
        'Noth the Plaguebringer', 'Heigan the Unclean', 'Loatheb', 'Patchwerk',
        'Grobbulus', 'Gluth', 'Thaddius', 'Instructor Razuvious', 'Gothik the Harvester',
        'The Four Horsemen', 'Sapphiron', "Kel'Thuzad"}),
    ('РС — Рубиновое святилище', {'Halion', 'General Zarithrian', 'Saviana Ragefire', 'Baltharus the Warborn'}),
    ('ОС — Обсидиановое святилище', {'Sartharion', 'Vesperon', 'Shadron', 'Tenebron'}),
    ('Око Вечности', {'Malygos'}),
    ('Логово Ониксии', {'Onyxia'}),
    ('Склеп Аркавона', {'Archavon the Stone Watcher', 'Emalon the Storm Watcher',
        'Koralon the Flame Watcher', 'Toravon the Ice Watcher'}),
    ('MC — Огненные Недра', {'Lucifron', 'Magmadar', 'Gehennas', 'Garr', 'Shazzrah',
        'Baron Geddon', 'Golemagg the Incinerator', 'Sulfuron Harbringer', 'Majordomo Executus', 'Ragnaros'}),
    ('BWL — Логово Крыла Тьмы', {'Razorgore the Untamed', 'Vaelastrasz the Corrupt',
        'Broodlord Lashlayer', 'Firemaw', 'Ebonroc', 'Flamegor', 'Chromaggus', 'Nefarian'}),
    ('ZG — Зул’Гуруб', {'High Priest Venoxis', 'High Priestess Jeklik', 'High Priestess Marli',
        'High Priest Thekal', 'Hight Priestess Arlokk', 'Hakkar', 'Bloodlord Mandokir',
        "Gahz'Rankha", "Jin'do the Hexxer"}),
)

def raid_label(fights):
    fights = set(map(str, fights))
    matches = [name for name, bosses in RAIDS if fights & bosses]
    return ' / '.join(matches) if matches else 'Рейдовый отчёт'
