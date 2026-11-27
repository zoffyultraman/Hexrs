//! Embedded cold-start dataset for offline <1ms Arena / ARAM recommendations.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

struct SeedChampion {
    hero_id: i32,
    name_zh: &'static str,
    title_zh: &'static str,
    alias: &'static str,
    tier: &'static str,
    win_rate: f64,
    pick_rate: f64,
    tags: &'static str,
    ad_weight: f64,
    ap_weight: f64,
    tank_score: f64,
    cc_score: f64,
}

struct SeedAugment {
    augment_id: i32,
    name_zh: &'static str,
    rarity: &'static str,
    phash: &'static str,
    desc_zh: &'static str,
    default_tier: &'static str,
    default_win_rate: f64,
    default_pick_rate: f64,
}

struct SeedDuo {
    hero_a: i32,
    hero_b: i32,
    duo_win_rate: f64,
    lift_score: f64,
    sample_size: i32,
}

struct SeedAugSynergy {
    hero_id: i32,
    aug_a: i32,
    aug_b: i32,
    lift_score: f64,
    sample_size: i32,
}

struct SeedBuild {
    hero_id: i32,
    playstyle_tag: &'static str,
    item_ids: [i32; 3],
    win_rate: f64,
}

const CHAMPIONS: &[SeedChampion] = &[
    SeedChampion {
        hero_id: 157,
        name_zh: "亚索",
        title_zh: "疾风剑豪",
        alias: "Yasuo",
        tier: "S+",
        win_rate: 56.8,
        pick_rate: 14.2,
        tags: r#"["暴击特效流","高机动"]"#,
        ad_weight: 0.90,
        ap_weight: 0.10,
        tank_score: 0.25,
        cc_score: 0.55,
    },
    SeedChampion {
        hero_id: 777,
        name_zh: "永恩",
        title_zh: "封魔剑魂",
        alias: "Yone",
        tier: "S",
        win_rate: 55.4,
        pick_rate: 11.8,
        tags: r#"["双修暴击","切后排"]"#,
        ad_weight: 0.72,
        ap_weight: 0.28,
        tank_score: 0.30,
        cc_score: 0.65,
    },
    SeedChampion {
        hero_id: 238,
        name_zh: "劫",
        title_zh: "影流之主",
        alias: "Zed",
        tier: "A",
        win_rate: 52.3,
        pick_rate: 9.4,
        tags: r#"["穿甲流","高爆发"]"#,
        ad_weight: 0.95,
        ap_weight: 0.05,
        tank_score: 0.10,
        cc_score: 0.15,
    },
    SeedChampion {
        hero_id: 145,
        name_zh: "卡莎",
        title_zh: "虚空之女",
        alias: "Kaisa",
        tier: "S+",
        win_rate: 57.1,
        pick_rate: 15.6,
        tags: r#"["双修特效流","拉扯输出"]"#,
        ad_weight: 0.58,
        ap_weight: 0.42,
        tank_score: 0.10,
        cc_score: 0.05,
    },
    SeedChampion {
        hero_id: 67,
        name_zh: "薇恩",
        title_zh: "暗夜猎手",
        alias: "Vayne",
        tier: "S",
        win_rate: 55.9,
        pick_rate: 10.7,
        tags: r#"["特效流","切坦核心"]"#,
        ad_weight: 0.92,
        ap_weight: 0.08,
        tank_score: 0.15,
        cc_score: 0.25,
    },
    SeedChampion {
        hero_id: 360,
        name_zh: "莎弥拉",
        title_zh: "沙漠玫瑰",
        alias: "Samira",
        tier: "S",
        win_rate: 54.8,
        pick_rate: 8.9,
        tags: r#"["暴击收割","进场AOE"]"#,
        ad_weight: 0.95,
        ap_weight: 0.05,
        tank_score: 0.20,
        cc_score: 0.15,
    },
    SeedChampion {
        hero_id: 82,
        name_zh: "莫德凯撒",
        title_zh: "铁铠冥魂",
        alias: "Mordekaiser",
        tier: "S",
        win_rate: 55.2,
        pick_rate: 8.4,
        tags: r#"["法坦重装","单挑领域"]"#,
        ad_weight: 0.10,
        ap_weight: 0.90,
        tank_score: 0.78,
        cc_score: 0.45,
    },
    SeedChampion {
        hero_id: 875,
        name_zh: "瑟提",
        title_zh: "腕豪",
        alias: "Sett",
        tier: "S+",
        win_rate: 57.4,
        pick_rate: 12.1,
        tags: r#"["血手重装","前排控制"]"#,
        ad_weight: 0.92,
        ap_weight: 0.08,
        tank_score: 0.85,
        cc_score: 0.72,
    },
    SeedChampion {
        hero_id: 122,
        name_zh: "德莱厄斯",
        title_zh: "诺克萨斯之手",
        alias: "Darius",
        tier: "A",
        win_rate: 53.1,
        pick_rate: 7.9,
        tags: r#"["重装战士","持续流血"]"#,
        ad_weight: 0.96,
        ap_weight: 0.04,
        tank_score: 0.72,
        cc_score: 0.48,
    },
    SeedChampion {
        hero_id: 54,
        name_zh: "墨菲特",
        title_zh: "熔岩巨兽",
        alias: "Malphite",
        tier: "S",
        win_rate: 55.6,
        pick_rate: 9.8,
        tags: r#"["前排控制","先手开团"]"#,
        ad_weight: 0.15,
        ap_weight: 0.85,
        tank_score: 0.95,
        cc_score: 0.88,
    },
    SeedChampion {
        hero_id: 32,
        name_zh: "阿木木",
        title_zh: "殇之木乃伊",
        alias: "Amumu",
        tier: "S+",
        win_rate: 58.2,
        pick_rate: 9.1,
        tags: r#"["前排控制","法坦团控"]"#,
        ad_weight: 0.10,
        ap_weight: 0.90,
        tank_score: 0.90,
        cc_score: 0.96,
    },
    SeedChampion {
        hero_id: 201,
        name_zh: "布隆",
        title_zh: "弗雷尔卓德之心",
        alias: "Braum",
        tier: "A",
        win_rate: 53.6,
        pick_rate: 5.4,
        tags: r#"["反手保护","前排控制"]"#,
        ad_weight: 0.12,
        ap_weight: 0.88,
        tank_score: 0.92,
        cc_score: 0.82,
    },
    SeedChampion {
        hero_id: 89,
        name_zh: "蕾欧娜",
        title_zh: "曙光女神",
        alias: "Leona",
        tier: "S",
        win_rate: 55.8,
        pick_rate: 7.3,
        tags: r#"["硬控开团","前排承伤"]"#,
        ad_weight: 0.15,
        ap_weight: 0.85,
        tank_score: 0.94,
        cc_score: 0.98,
    },
    SeedChampion {
        hero_id: 63,
        name_zh: "布兰德",
        title_zh: "复仇焰魂",
        alias: "Brand",
        tier: "S+",
        win_rate: 57.9,
        pick_rate: 11.4,
        tags: r#"["灼烧法伤","高额AOE"]"#,
        ad_weight: 0.05,
        ap_weight: 0.95,
        tank_score: 0.12,
        cc_score: 0.42,
    },
    SeedChampion {
        hero_id: 112,
        name_zh: "维克托",
        title_zh: "机械先驱",
        alias: "Viktor",
        tier: "S",
        win_rate: 54.7,
        pick_rate: 8.2,
        tags: r#"["法穿爆发","阵地控制"]"#,
        ad_weight: 0.05,
        ap_weight: 0.95,
        tank_score: 0.15,
        cc_score: 0.50,
    },
    SeedChampion {
        hero_id: 99,
        name_zh: "拉克丝",
        title_zh: "光辉女郎",
        alias: "Lux",
        tier: "A",
        win_rate: 52.8,
        pick_rate: 10.5,
        tags: r#"["Poke消耗","远程控制"]"#,
        ad_weight: 0.05,
        ap_weight: 0.95,
        tank_score: 0.10,
        cc_score: 0.60,
    },
    SeedChampion {
        hero_id: 101,
        name_zh: "泽拉斯",
        title_zh: "远古巫灵",
        alias: "Xerath",
        tier: "A",
        win_rate: 52.4,
        pick_rate: 7.1,
        tags: r#"["超远Poke","法穿爆发"]"#,
        ad_weight: 0.05,
        ap_weight: 0.95,
        tank_score: 0.05,
        cc_score: 0.45,
    },
    SeedChampion {
        hero_id: 876,
        name_zh: "莉莉娅",
        title_zh: "含羞蓓蕾",
        alias: "Lillia",
        tier: "S",
        win_rate: 55.1,
        pick_rate: 6.8,
        tags: r#"["持续拉扯","群体睡眠"]"#,
        ad_weight: 0.08,
        ap_weight: 0.92,
        tank_score: 0.45,
        cc_score: 0.68,
    },
    SeedChampion {
        hero_id: 141,
        name_zh: "凯隐",
        title_zh: "影流之镰",
        alias: "Kayn",
        tier: "A",
        win_rate: 53.5,
        pick_rate: 8.6,
        tags: r#"["穿甲刺客","红凯重装"]"#,
        ad_weight: 0.95,
        ap_weight: 0.05,
        tank_score: 0.55,
        cc_score: 0.48,
    },
    SeedChampion {
        hero_id: 887,
        name_zh: "格温",
        title_zh: "灵罗娃娃",
        alias: "Gwen",
        tier: "S",
        win_rate: 55.0,
        pick_rate: 7.4,
        tags: r#"["AP切坦","免伤进场"]"#,
        ad_weight: 0.10,
        ap_weight: 0.90,
        tank_score: 0.48,
        cc_score: 0.20,
    },
    SeedChampion {
        hero_id: 498,
        name_zh: "霞",
        title_zh: "逆羽",
        alias: "Xayah",
        tier: "A",
        win_rate: 53.2,
        pick_rate: 6.9,
        tags: r#"["暴击反手","倒钩控制"]"#,
        ad_weight: 0.95,
        ap_weight: 0.05,
        tank_score: 0.12,
        cc_score: 0.45,
    },
    SeedChampion {
        hero_id: 497,
        name_zh: "洛",
        title_zh: "幻翎",
        alias: "Rakan",
        tier: "S",
        win_rate: 54.6,
        pick_rate: 6.2,
        tags: r#"["强开团控","灵活护盾"]"#,
        ad_weight: 0.10,
        ap_weight: 0.90,
        tank_score: 0.68,
        cc_score: 0.92,
    },
    SeedChampion {
        hero_id: 64,
        name_zh: "李青",
        title_zh: "盲僧",
        alias: "LeeSin",
        tier: "B",
        win_rate: 50.8,
        pick_rate: 9.7,
        tags: r#"["战士穿甲","回旋踢开团"]"#,
        ad_weight: 0.90,
        ap_weight: 0.10,
        tank_score: 0.48,
        cc_score: 0.58,
    },
    SeedChampion {
        hero_id: 55,
        name_zh: "卡特琳娜",
        title_zh: "不祥之刃",
        alias: "Katarina",
        tier: "A",
        win_rate: 52.9,
        pick_rate: 7.5,
        tags: r#"["双修特效","残局收割"]"#,
        ad_weight: 0.35,
        ap_weight: 0.65,
        tank_score: 0.15,
        cc_score: 0.05,
    },
    SeedChampion {
        hero_id: 119,
        name_zh: "德莱文",
        title_zh: "荣耀行刑官",
        alias: "Draven",
        tier: "A",
        win_rate: 52.7,
        pick_rate: 6.4,
        tags: r#"["高额暴击","穿甲碾压"]"#,
        ad_weight: 0.98,
        ap_weight: 0.02,
        tank_score: 0.10,
        cc_score: 0.22,
    },
];

const AUGMENTS: &[SeedAugment] = &[
    SeedAugment {
        augment_id: 1001,
        name_zh: "珠光护手",
        rarity: "棱彩",
        phash: "a5a55a5a3c3cc3c3",
        desc_zh: "你的技能可以暴击，并基于法术强度提供额外暴击几率。",
        default_tier: "S+",
        default_win_rate: 58.6,
        default_pick_rate: 19.4,
    },
    SeedAugment {
        augment_id: 1002,
        name_zh: "利刃华尔兹",
        rarity: "棱彩",
        phash: "0f0ff0f03333cccc",
        desc_zh: "变为不可阻挡的剑影快速斩击周围敌人 8 次。",
        default_tier: "S",
        default_win_rate: 56.1,
        default_pick_rate: 12.8,
    },
    SeedAugment {
        augment_id: 1003,
        name_zh: "双刀流",
        rarity: "棱彩",
        phash: "55aa55aa00ff00ff",
        desc_zh: "每次普攻附带一次额外攻击，大幅提高攻速与攻击特效触发频率。",
        default_tier: "S+",
        default_win_rate: 58.9,
        default_pick_rate: 16.5,
    },
    SeedAugment {
        augment_id: 1004,
        name_zh: "物法皆修",
        rarity: "棱彩",
        phash: "123456789abcdef0",
        desc_zh: "普攻与技能交替命中时额外造成高额双修自适应伤害。",
        default_tier: "S",
        default_win_rate: 55.3,
        default_pick_rate: 11.2,
    },
    SeedAugment {
        augment_id: 1005,
        name_zh: "逃跑计划",
        rarity: "棱彩",
        phash: "fedcba9876543210",
        desc_zh: "生命值危急时触发护盾并获得极高移速加成。",
        default_tier: "A",
        default_win_rate: 53.4,
        default_pick_rate: 8.7,
    },
    SeedAugment {
        augment_id: 1006,
        name_zh: "秘术冲拳",
        rarity: "棱彩",
        phash: "aa00bb11cc22dd33",
        desc_zh: "普攻命中敌人时按百分比大幅缩减所有基础技能剩余冷却。",
        default_tier: "S+",
        default_win_rate: 58.2,
        default_pick_rate: 17.1,
    },
    SeedAugment {
        augment_id: 1007,
        name_zh: "裁决使",
        rarity: "棱彩",
        phash: "11ee22dd33cc44bb",
        desc_zh: "参与击杀后立即刷新所有基础技能并获得爆发移速。",
        default_tier: "S",
        default_win_rate: 55.8,
        default_pick_rate: 13.0,
    },
    SeedAugment {
        augment_id: 1008,
        name_zh: "回归基本功",
        rarity: "黄金",
        phash: "f00f0ff0a11a1aa1",
        desc_zh: "封印终极技能，大幅提高基础技能伤害、技能急速与治疗护盾。",
        default_tier: "S",
        default_win_rate: 55.5,
        default_pick_rate: 10.6,
    },
    SeedAugment {
        augment_id: 1009,
        name_zh: "吸血习性",
        rarity: "黄金",
        phash: "778899aabbccdde0",
        desc_zh: "获得高额全能吸血，溢出治疗量可转化为护盾。",
        default_tier: "S",
        default_win_rate: 55.1,
        default_pick_rate: 14.3,
    },
    SeedAugment {
        augment_id: 1010,
        name_zh: "玻璃大炮",
        rarity: "黄金",
        phash: "01234567fedcba98",
        desc_zh: "最大生命值降低 30%，造成的全部伤害显著提高。",
        default_tier: "A",
        default_win_rate: 53.9,
        default_pick_rate: 9.8,
    },
    SeedAugment {
        augment_id: 1011,
        name_zh: "缩小引擎",
        rarity: "黄金",
        phash: "89abcdef01234567",
        desc_zh: "缩减英雄体型并大幅增加技能急速与移动速度。",
        default_tier: "A",
        default_win_rate: 54.0,
        default_pick_rate: 11.4,
    },
    SeedAugment {
        augment_id: 1012,
        name_zh: "小丑学院",
        rarity: "黄金",
        phash: "c3c33c3c5a5aa5a5",
        desc_zh: "阵亡后自爆造成高额真实伤害并缩短复活等待时间。",
        default_tier: "B",
        default_win_rate: 51.6,
        default_pick_rate: 6.4,
    },
    SeedAugment {
        augment_id: 1013,
        name_zh: "地震",
        rarity: "黄金",
        phash: "2468ace013579bdf",
        desc_zh: "使用位移或闪现技能后在落点制造震荡波，击飞并伤害敌人。",
        default_tier: "S",
        default_win_rate: 55.7,
        default_pick_rate: 12.1,
    },
    SeedAugment {
        augment_id: 1014,
        name_zh: "坦克引擎",
        rarity: "黄金",
        phash: "13579bdf2468ace0",
        desc_zh: "每次参与击杀永久叠加最大生命值与体格。",
        default_tier: "S",
        default_win_rate: 56.0,
        default_pick_rate: 13.7,
    },
    SeedAugment {
        augment_id: 1015,
        name_zh: "踢踏舞",
        rarity: "棱彩",
        phash: "6699aa55cc33ff00",
        desc_zh: "每次普通攻击永久叠加攻击速度与移动速度。",
        default_tier: "S+",
        default_win_rate: 57.8,
        default_pick_rate: 15.2,
    },
    SeedAugment {
        augment_id: 1016,
        name_zh: "巨人杀手",
        rarity: "黄金",
        phash: "996655aa33cc00ff",
        desc_zh: "体型缩减，对最大生命值高于自己的敌人造成额外真实感百分比伤害。",
        default_tier: "A",
        default_win_rate: 54.4,
        default_pick_rate: 10.9,
    },
    SeedAugment {
        augment_id: 1017,
        name_zh: "歌利亚巨人",
        rarity: "棱彩",
        phash: "0f3c5a6996a5c3f0",
        desc_zh: "体型增大，额外获得 35% 最大生命值与自适应之力。",
        default_tier: "S+",
        default_win_rate: 58.1,
        default_pick_rate: 16.0,
    },
    SeedAugment {
        augment_id: 1018,
        name_zh: "魔鬼之舞",
        rarity: "棱彩",
        phash: "f0c3a596695a3c0f",
        desc_zh: "生命偷取与法力续航大幅增强，高血量时获得额外自适应之力。",
        default_tier: "A",
        default_win_rate: 53.7,
        default_pick_rate: 8.3,
    },
    SeedAugment {
        augment_id: 1019,
        name_zh: "心灵净化",
        rarity: "白银",
        phash: "33cc33cc66996699",
        desc_zh: "提供 35% 韧性，并在受到强控后短暂免疫后续控制。",
        default_tier: "B",
        default_win_rate: 51.9,
        default_pick_rate: 7.6,
    },
    SeedAugment {
        augment_id: 1020,
        name_zh: "灵巧",
        rarity: "白银",
        phash: "44bb44bb88778877",
        desc_zh: "直接提供 60% 攻击速度加成。",
        default_tier: "A",
        default_win_rate: 53.2,
        default_pick_rate: 11.5,
    },
    SeedAugment {
        augment_id: 1021,
        name_zh: "冰霜幽灵",
        rarity: "白银",
        phash: "deadbeef12345678",
        desc_zh: "每隔数秒自动释放寒冰新星，对周围敌人施加禁锢效果。",
        default_tier: "A",
        default_win_rate: 53.8,
        default_pick_rate: 9.2,
    },
    SeedAugment {
        augment_id: 1022,
        name_zh: "闪电打击",
        rarity: "棱彩",
        phash: "87654321beefdead",
        desc_zh: "攻速超过阈值后，每次普攻都会发射连锁魔法闪电。",
        default_tier: "S+",
        default_win_rate: 58.4,
        default_pick_rate: 15.8,
    },
];

const DUO_SYNERGIES: &[SeedDuo] = &[
    SeedDuo { hero_a: 54, hero_b: 157, duo_win_rate: 59.1, lift_score: 1.24, sample_size: 420 },
    SeedDuo { hero_a: 157, hero_b: 777, duo_win_rate: 56.4, lift_score: 1.18, sample_size: 380 },
    SeedDuo { hero_a: 32, hero_b: 157, duo_win_rate: 57.6, lift_score: 1.20, sample_size: 265 },
    SeedDuo { hero_a: 89, hero_b: 157, duo_win_rate: 56.8, lift_score: 1.19, sample_size: 210 },
    SeedDuo { hero_a: 157, hero_b: 497, duo_win_rate: 56.2, lift_score: 1.17, sample_size: 195 },
    SeedDuo { hero_a: 64, hero_b: 157, duo_win_rate: 55.4, lift_score: 1.15, sample_size: 290 },
    SeedDuo { hero_a: 497, hero_b: 498, duo_win_rate: 59.4, lift_score: 1.28, sample_size: 510 },
    SeedDuo { hero_a: 89, hero_b: 145, duo_win_rate: 56.9, lift_score: 1.19, sample_size: 330 },
    SeedDuo { hero_a: 32, hero_b: 145, duo_win_rate: 56.5, lift_score: 1.17, sample_size: 280 },
    SeedDuo { hero_a: 89, hero_b: 360, duo_win_rate: 58.1, lift_score: 1.23, sample_size: 350 },
    SeedDuo { hero_a: 32, hero_b: 360, duo_win_rate: 57.6, lift_score: 1.21, sample_size: 290 },
    SeedDuo { hero_a: 201, hero_b: 360, duo_win_rate: 55.8, lift_score: 1.16, sample_size: 240 },
    SeedDuo { hero_a: 67, hero_b: 201, duo_win_rate: 57.4, lift_score: 1.20, sample_size: 310 },
    SeedDuo { hero_a: 89, hero_b: 119, duo_win_rate: 56.2, lift_score: 1.17, sample_size: 220 },
    SeedDuo { hero_a: 119, hero_b: 201, duo_win_rate: 55.9, lift_score: 1.16, sample_size: 205 },
    SeedDuo { hero_a: 122, hero_b: 201, duo_win_rate: 55.6, lift_score: 1.15, sample_size: 180 },
    SeedDuo { hero_a: 32, hero_b: 875, duo_win_rate: 56.3, lift_score: 1.18, sample_size: 230 },
    SeedDuo { hero_a: 32, hero_b: 63, duo_win_rate: 58.2, lift_score: 1.22, sample_size: 340 },
    SeedDuo { hero_a: 54, hero_b: 112, duo_win_rate: 56.7, lift_score: 1.18, sample_size: 260 },
    SeedDuo { hero_a: 99, hero_b: 101, duo_win_rate: 56.0, lift_score: 1.16, sample_size: 295 },
    SeedDuo { hero_a: 82, hero_b: 876, duo_win_rate: 57.1, lift_score: 1.19, sample_size: 245 },
    SeedDuo { hero_a: 875, hero_b: 887, duo_win_rate: 56.4, lift_score: 1.17, sample_size: 215 },
    SeedDuo { hero_a: 141, hero_b: 497, duo_win_rate: 55.7, lift_score: 1.16, sample_size: 175 },
    SeedDuo { hero_a: 32, hero_b: 55, duo_win_rate: 57.3, lift_score: 1.21, sample_size: 270 },
    SeedDuo { hero_a: 54, hero_b: 777, duo_win_rate: 57.8, lift_score: 1.21, sample_size: 315 },
];

const AUGMENT_SYNERGIES: &[SeedAugSynergy] = &[
    // Generic cross-hero synergies (hero_id = 0) so every champion benefits
    SeedAugSynergy { hero_id: 0, aug_a: 1003, aug_b: 1022, lift_score: 1.32, sample_size: 460 },
    SeedAugSynergy { hero_id: 0, aug_a: 1003, aug_b: 1015, lift_score: 1.26, sample_size: 390 },
    SeedAugSynergy { hero_id: 0, aug_a: 1001, aug_b: 1010, lift_score: 1.28, sample_size: 340 },
    SeedAugSynergy { hero_id: 0, aug_a: 1001, aug_b: 1008, lift_score: 1.21, sample_size: 280 },
    SeedAugSynergy { hero_id: 0, aug_a: 1006, aug_b: 1013, lift_score: 1.25, sample_size: 310 },
    SeedAugSynergy { hero_id: 0, aug_a: 1006, aug_b: 1002, lift_score: 1.22, sample_size: 245 },
    SeedAugSynergy { hero_id: 0, aug_a: 1014, aug_b: 1017, lift_score: 1.31, sample_size: 520 },
    SeedAugSynergy { hero_id: 0, aug_a: 1009, aug_b: 1017, lift_score: 1.19, sample_size: 270 },
    SeedAugSynergy { hero_id: 0, aug_a: 1002, aug_b: 1007, lift_score: 1.23, sample_size: 220 },
    SeedAugSynergy { hero_id: 0, aug_a: 1015, aug_b: 1020, lift_score: 1.20, sample_size: 295 },
    SeedAugSynergy { hero_id: 0, aug_a: 1013, aug_b: 1021, lift_score: 1.18, sample_size: 165 },
    // Hero-specific marquee synergies
    SeedAugSynergy { hero_id: 157, aug_a: 1003, aug_b: 1022, lift_score: 1.36, sample_size: 185 },
    SeedAugSynergy { hero_id: 157, aug_a: 1006, aug_b: 1013, lift_score: 1.29, sample_size: 140 },
    SeedAugSynergy { hero_id: 145, aug_a: 1003, aug_b: 1022, lift_score: 1.35, sample_size: 210 },
    SeedAugSynergy { hero_id: 145, aug_a: 1004, aug_b: 1015, lift_score: 1.27, sample_size: 150 },
    SeedAugSynergy { hero_id: 63, aug_a: 1001, aug_b: 1010, lift_score: 1.33, sample_size: 195 },
    SeedAugSynergy { hero_id: 875, aug_a: 1014, aug_b: 1017, lift_score: 1.34, sample_size: 230 },
    SeedAugSynergy { hero_id: 32, aug_a: 1014, aug_b: 1021, lift_score: 1.26, sample_size: 145 },
];

const HERO_BUILDS: &[SeedBuild] = &[
    SeedBuild { hero_id: 157, playstyle_tag: "暴击特效流", item_ids: [3153, 6673, 3031], win_rate: 57.6 },
    SeedBuild { hero_id: 157, playstyle_tag: "半肉吸血流", item_ids: [3153, 3074, 3053], win_rate: 55.4 },
    SeedBuild { hero_id: 777, playstyle_tag: "暴击特效流", item_ids: [3153, 6673, 3031], win_rate: 56.2 },
    SeedBuild { hero_id: 238, playstyle_tag: "穿甲秒杀流", item_ids: [6692, 3142, 6694], win_rate: 53.8 },
    SeedBuild { hero_id: 145, playstyle_tag: "双修特效流", item_ids: [3124, 3115, 3089], win_rate: 58.1 },
    SeedBuild { hero_id: 145, playstyle_tag: "暴击攻速流", item_ids: [6672, 3124, 3031], win_rate: 56.0 },
    SeedBuild { hero_id: 67, playstyle_tag: "攻速特效流", item_ids: [3153, 3124, 3091], win_rate: 56.9 },
    SeedBuild { hero_id: 360, playstyle_tag: "暴击收割流", item_ids: [6676, 3031, 6673], win_rate: 55.7 },
    SeedBuild { hero_id: 82, playstyle_tag: "法坦续航流", item_ids: [3116, 4633, 3157], win_rate: 56.0 },
    SeedBuild { hero_id: 875, playstyle_tag: "心之钢重装", item_ids: [3084, 3053, 3748], win_rate: 58.3 },
    SeedBuild { hero_id: 122, playstyle_tag: "重装战士流", item_ids: [3078, 3053, 3742], win_rate: 54.1 },
    SeedBuild { hero_id: 54, playstyle_tag: "纯甲开团流", item_ids: [3068, 3075, 3110], win_rate: 56.4 },
    SeedBuild { hero_id: 54, playstyle_tag: "核弹AP流", item_ids: [3152, 4645, 3089], win_rate: 54.0 },
    SeedBuild { hero_id: 32, playstyle_tag: "灼烧法坦流", item_ids: [3068, 6653, 3075], win_rate: 58.8 },
    SeedBuild { hero_id: 201, playstyle_tag: "辅助护卫流", item_ids: [3190, 3109, 3075], win_rate: 54.2 },
    SeedBuild { hero_id: 89, playstyle_tag: "硬控承伤流", item_ids: [3068, 3075, 3190], win_rate: 56.5 },
    SeedBuild { hero_id: 63, playstyle_tag: "双面具灼烧", item_ids: [6653, 3116, 3089], win_rate: 58.7 },
    SeedBuild { hero_id: 112, playstyle_tag: "法穿爆发流", item_ids: [6655, 4645, 3089], win_rate: 55.4 },
    SeedBuild { hero_id: 99, playstyle_tag: "法穿Poke流", item_ids: [6655, 4645, 3089], win_rate: 53.7 },
    SeedBuild { hero_id: 101, playstyle_tag: "法穿炮台流", item_ids: [6655, 4645, 3089], win_rate: 53.4 },
    SeedBuild { hero_id: 876, playstyle_tag: "持续拉扯流", item_ids: [6653, 4633, 3157], win_rate: 56.1 },
    SeedBuild { hero_id: 141, playstyle_tag: "穿甲刺客流", item_ids: [3142, 6692, 6694], win_rate: 54.3 },
    SeedBuild { hero_id: 887, playstyle_tag: "AP切坦流", item_ids: [3115, 4633, 3089], win_rate: 55.9 },
    SeedBuild { hero_id: 498, playstyle_tag: "暴击收割流", item_ids: [6672, 6675, 3031], win_rate: 54.1 },
    SeedBuild { hero_id: 497, playstyle_tag: "先手开团流", item_ids: [3190, 3157, 3050], win_rate: 55.3 },
    SeedBuild { hero_id: 64, playstyle_tag: "穿甲战士流", item_ids: [6692, 3071, 3053], win_rate: 51.9 },
    SeedBuild { hero_id: 55, playstyle_tag: "双修收割流", item_ids: [3115, 4645, 3157], win_rate: 53.8 },
    SeedBuild { hero_id: 119, playstyle_tag: "高额暴击流", item_ids: [6676, 3031, 3072], win_rate: 53.6 },
];

/// Returns the Chinese display name for common seeded item IDs.
pub fn lookup_item_name_zh(item_id: i32) -> &'static str {
    match item_id {
        3031 => "无尽之刃",
        3050 => "基克的聚合",
        3053 => "斯特拉克的挑战护手",
        3068 => "日炎圣盾",
        3071 => "黑色切割者",
        3072 => "饮血剑",
        3074 => "贪欲九头蛇",
        3075 => "荆棘之甲",
        3078 => "三相之力",
        3084 => "心之钢",
        3089 => "灭世者的死亡之帽",
        3091 => "智慧末刃",
        3109 => "骑士之誓",
        3110 => "冰霜之心",
        3115 => "纳什之牙",
        3116 => "瑞莱的冰晶节杖",
        3124 => "鬼索的狂暴之刃",
        3142 => "幽梦之灵",
        3152 => "海克斯科技火箭腰带",
        3153 => "破败王者之刃",
        3157 => "中娅沙漏",
        3190 => "钢铁烈阳之匣",
        3742 => "亡者的板甲",
        3748 => "巨型九头蛇",
        4633 => "峡谷制造者",
        4645 => "影焰",
        6653 => "兰德里的折磨",
        6655 => "卢登的伙伴",
        6672 => "海妖杀手",
        6673 => "不朽盾弓",
        6675 => "纳沃利迅刃",
        6676 => "收集者",
        6692 => "星蚀",
        6694 => "赛瑞尔达的怨恨",
        _ => "核心装备",
    }
}

/// Populates `meta.db` with the embedded cold-start champions, augments, synergies, and builds.
pub fn populate_seed_data(conn: &Connection) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE;")
        .context("failed to begin seed transaction")?;

    let result = populate_inner(conn);
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT;")
                .context("failed to commit seed transaction")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(err)
        }
    }
}

fn populate_inner(conn: &Connection) -> Result<()> {
    for champ in CHAMPIONS {
        conn.execute(
            "INSERT OR REPLACE INTO champions (hero_id, name_zh, title_zh, alias) VALUES (?1, ?2, ?3, ?4)",
            params![champ.hero_id, champ.name_zh, champ.title_zh, champ.alias],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO hero_tier_stats (hero_id, tier, win_rate, pick_rate, tags) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![champ.hero_id, champ.tier, champ.win_rate, champ.pick_rate, champ.tags],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO hero_attributes (hero_id, ad_weight, ap_weight, tank_score, cc_score) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![champ.hero_id, champ.ad_weight, champ.ap_weight, champ.tank_score, champ.cc_score],
        )?;
    }

    for aug in AUGMENTS {
        conn.execute(
            "INSERT OR REPLACE INTO augment_meta (augment_id, name_zh, rarity, phash, desc_zh) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![aug.augment_id, aug.name_zh, aug.rarity, aug.phash, aug.desc_zh],
        )?;
    }

    // Seed per-hero augment stats tailored to champion archetypes.
    for champ in CHAMPIONS {
        let preferred: &[i32] = if champ.tank_score >= 0.70 {
            &[1014, 1017, 1013, 1021, 1009, 1019]
        } else if champ.ap_weight >= 0.70 {
            &[1001, 1010, 1008, 1011, 1004, 1005]
        } else {
            &[1003, 1022, 1015, 1006, 1002, 1007, 1009, 1020]
        };

        for aug in AUGMENTS {
            let bonus = preferred
                .iter()
                .position(|&id| id == aug.augment_id)
                .map(|idx| 2.4 - (idx as f64) * 0.35)
                .unwrap_or(-0.8);
            let win_rate = ((aug.default_win_rate + bonus) * 10.0).round() / 10.0;
            let tier = if win_rate >= 57.5 {
                "S+"
            } else if win_rate >= 55.2 {
                "S"
            } else if win_rate >= 53.0 {
                "A"
            } else if win_rate >= 51.0 {
                "B"
            } else {
                aug.default_tier
            };

            conn.execute(
                "INSERT OR REPLACE INTO hero_augment_stats (hero_id, augment_id, tier, win_rate, pick_rate, sample_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![champ.hero_id, aug.augment_id, tier, win_rate, aug.default_pick_rate, 200_i32],
            )?;
        }
    }

    for duo in DUO_SYNERGIES {
        let (a, b) = if duo.hero_a <= duo.hero_b {
            (duo.hero_a, duo.hero_b)
        } else {
            (duo.hero_b, duo.hero_a)
        };
        conn.execute(
            "INSERT OR REPLACE INTO hero_duo_synergy (hero_a, hero_b, duo_win_rate, lift_score, sample_size) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![a, b, duo.duo_win_rate, duo.lift_score, duo.sample_size],
        )?;
    }

    for syn in AUGMENT_SYNERGIES {
        let (a, b) = if syn.aug_a <= syn.aug_b {
            (syn.aug_a, syn.aug_b)
        } else {
            (syn.aug_b, syn.aug_a)
        };
        conn.execute(
            "INSERT OR REPLACE INTO augment_synergy (hero_id, aug_a, aug_b, lift_score, sample_size) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![syn.hero_id, a, b, syn.lift_score, syn.sample_size],
        )?;
    }

    conn.execute("DELETE FROM hero_builds", [])?;
    for build in HERO_BUILDS {
        let items_json = serde_json::json!([
            { "id": build.item_ids[0], "name_zh": lookup_item_name_zh(build.item_ids[0]) },
            { "id": build.item_ids[1], "name_zh": lookup_item_name_zh(build.item_ids[1]) },
            { "id": build.item_ids[2], "name_zh": lookup_item_name_zh(build.item_ids[2]) },
        ])
        .to_string();

        conn.execute(
            "INSERT INTO hero_builds (hero_id, playstyle_tag, core_items_json, win_rate) VALUES (?1, ?2, ?3, ?4)",
            params![build.hero_id, build.playstyle_tag, items_json, build.win_rate],
        )?;
    }

    conn.execute(
        "INSERT OR REPLACE INTO meta_info (key, value) VALUES ('seed_version', '1')",
        [],
    )?;

    Ok(())
}
