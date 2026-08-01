//! 每日组稿纯函数：只分配素材 id，不读写数据库或文件系统。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkuPool {
    pub sku_id: i64,
    pub tier: String,
    pub image_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPool {
    pub title_ids: Vec<i64>,
    pub body_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicCandidate {
    pub scope: String,
    pub sku_ids: Vec<i64>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ComposeInput {
    pub posts_per_day: usize,
    pub images_per_post: usize,
    pub mixed_count: usize,
    pub skus: Vec<SkuPool>,
    pub texts: TextPool,
    pub topics: Vec<TopicCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Shortage {
    pub kind: String,
    pub needed: usize,
    pub available: usize,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedPost {
    pub seq: usize,
    pub kind: String,
    pub sku_ids: Vec<i64>,
    pub image_ids: Vec<i64>,
    pub title_id: i64,
    pub body_id: i64,
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeOutput {
    pub posts: Vec<ComposedPost>,
    pub shortages: Vec<Shortage>,
}

fn tier_rank(tier: &str) -> u8 {
    match tier {
        "hot" => 0,
        "warm" => 1,
        _ => 2,
    }
}

fn topics_for(candidates: &[TopicCandidate], sku_ids: &[i64]) -> Vec<String> {
    let mut wanted = sku_ids.to_vec();
    wanted.sort_unstable();
    let exact = candidates.iter().find(|c| {
        if c.scope != "combo" {
            return false;
        }
        let mut ids = c.sku_ids.clone();
        ids.sort_unstable();
        ids == wanted
    });
    exact
        .or_else(|| candidates.iter().find(|c| c.scope == "product"))
        .or_else(|| candidates.iter().find(|c| c.scope == "general"))
        .map(|c| c.tags.clone())
        .unwrap_or_default()
}

pub fn compose(input: &ComposeInput) -> ComposeOutput {
    let target = input
        .posts_per_day
        .min(input.texts.title_ids.len())
        .min(input.texts.body_ids.len());
    let mut shortages = Vec::new();
    if target < input.posts_per_day {
        shortages.push(Shortage {
            kind: "copy".into(),
            needed: input.posts_per_day * 2,
            available: input.texts.title_ids.len() + input.texts.body_ids.len(),
            detail: "商品可用标题或正文不足".into(),
        });
    }

    let mut ordered = input.skus.clone();
    ordered.sort_by_key(|s| (tier_rank(&s.tier), s.sku_id));
    for sku in ordered
        .iter()
        .filter(|sku| sku.image_ids.len() < input.images_per_post)
    {
        shortages.push(Shortage {
            kind: "skuImages".into(),
            needed: input.images_per_post,
            available: sku.image_ids.len(),
            detail: format!("SKU {} 图片不足，当天不参与组稿", sku.sku_id),
        });
    }
    ordered.retain(|sku| sku.image_ids.len() >= input.images_per_post);
    let mut images: HashMap<i64, Vec<i64>> = ordered
        .iter()
        .map(|s| (s.sku_id, s.image_ids.clone()))
        .collect();
    let mut posts = Vec::new();
    let mut single_cursor = 0usize;

    for seq in 0..target {
        let wants_mixed = seq < input.mixed_count;
        let (kind, sku_ids, image_ids) = if wants_mixed {
            let picked: Vec<i64> = ordered
                .iter()
                .filter(|s| images.get(&s.sku_id).is_some_and(|v| !v.is_empty()))
                .take(input.images_per_post)
                .map(|s| s.sku_id)
                .collect();
            if picked.len() < input.images_per_post {
                shortages.push(Shortage {
                    kind: "mixedImages".into(),
                    needed: input.images_per_post,
                    available: picked.len(),
                    detail: "混合篇需要足够多的不同 SKU".into(),
                });
                break;
            }
            let mut picked_images = Vec::with_capacity(picked.len());
            for sku_id in &picked {
                if let Some(pool) = images.get_mut(sku_id) {
                    if !pool.is_empty() {
                        picked_images.push(pool.remove(0));
                    }
                }
            }
            ("mixed".to_string(), picked, picked_images)
        } else {
            let picked_index = (0..ordered.len())
                .map(|offset| (single_cursor + offset) % ordered.len())
                .find(|index| {
                    images
                        .get(&ordered[*index].sku_id)
                        .is_some_and(|pool| pool.len() >= input.images_per_post)
                });
            let Some(picked_index) = picked_index else {
                let available = images.values().map(Vec::len).max().unwrap_or(0);
                shortages.push(Shortage {
                    kind: "singleImages".into(),
                    needed: input.images_per_post,
                    available,
                    detail: "没有 SKU 还够组成一篇单款图文".into(),
                });
                break;
            };
            let sku_id = ordered[picked_index].sku_id;
            single_cursor = (picked_index + 1) % ordered.len();
            let picked = images
                .get_mut(&sku_id)
                .map(|pool| pool.drain(0..input.images_per_post).collect())
                .unwrap_or_default();
            ("single".to_string(), vec![sku_id], picked)
        };

        posts.push(ComposedPost {
            seq,
            kind,
            topics: topics_for(&input.topics, &sku_ids),
            sku_ids,
            image_ids,
            title_id: input.texts.title_ids[seq],
            body_id: input.texts.body_ids[seq],
        });
    }

    let unique_images: HashSet<i64> = posts
        .iter()
        .flat_map(|p| p.image_ids.iter().copied())
        .collect();
    debug_assert_eq!(
        unique_images.len(),
        posts.iter().map(|p| p.image_ids.len()).sum::<usize>()
    );
    ComposeOutput { posts, shortages }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;

    fn input() -> ComposeInput {
        ComposeInput {
            posts_per_day: 3,
            images_per_post: 2,
            mixed_count: 1,
            skus: vec![
                SkuPool {
                    sku_id: 1,
                    tier: "hot".into(),
                    image_ids: vec![1, 2, 3, 4, 5],
                },
                SkuPool {
                    sku_id: 2,
                    tier: "warm".into(),
                    image_ids: vec![6, 7, 8, 9, 10],
                },
            ],
            texts: TextPool {
                title_ids: vec![11, 12, 13],
                body_ids: vec![21, 22, 23],
            },
            topics: vec![
                TopicCandidate {
                    scope: "general".into(),
                    sku_ids: vec![],
                    tags: vec!["#通用".into()],
                },
                TopicCandidate {
                    scope: "product".into(),
                    sku_ids: vec![],
                    tags: vec!["#商品".into()],
                },
                TopicCandidate {
                    scope: "combo".into(),
                    sku_ids: vec![2, 1],
                    tags: vec!["#组合".into()],
                },
            ],
        }
    }

    #[test]
    fn mixed_uses_distinct_skus_and_all_material_is_unique() {
        let out = compose(&input());
        assert_eq!(out.posts.len(), 3);
        assert_eq!(out.posts[0].sku_ids, vec![1, 2]);
        assert_eq!(out.posts[0].topics, vec!["#组合"]);
        let ids: Vec<i64> = out.posts.iter().flat_map(|p| p.image_ids.clone()).collect();
        assert_eq!(ids.len(), ids.iter().copied().collect::<HashSet<_>>().len());
        assert_eq!(
            out.posts
                .iter()
                .map(|p| p.title_id)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        assert_eq!(
            out.posts
                .iter()
                .map(|p| p.body_id)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn shrinking_eligible_pool_does_not_skip_the_warm_tier() {
        let output = compose(&ComposeInput {
            posts_per_day: 2,
            images_per_post: 1,
            mixed_count: 0,
            skus: vec![
                SkuPool {
                    sku_id: 1,
                    tier: "hot".into(),
                    image_ids: vec![1],
                },
                SkuPool {
                    sku_id: 2,
                    tier: "warm".into(),
                    image_ids: vec![2],
                },
                SkuPool {
                    sku_id: 3,
                    tier: "cold".into(),
                    image_ids: vec![3],
                },
            ],
            texts: TextPool {
                title_ids: vec![10, 11],
                body_ids: vec![20, 21],
            },
            topics: vec![],
        });
        assert_eq!(output.posts[0].sku_ids, vec![1]);
        assert_eq!(output.posts[1].sku_ids, vec![2]);
    }

    #[test]
    fn shortage_is_explicit_instead_of_reusing_material() {
        let mut x = input();
        x.posts_per_day = 8;
        let out = compose(&x);
        assert!(!out.shortages.is_empty());
        assert!(out.posts.len() < 8);
    }

    #[test]
    fn sku_below_one_post_capacity_is_skipped_even_for_mixed_posts() {
        let mut input = input();
        input.posts_per_day = 1;
        input.images_per_post = 2;
        input.mixed_count = 1;
        input.skus.push(SkuPool {
            sku_id: 3,
            tier: "hot".into(),
            image_ids: vec![99],
        });

        let out = compose(&input);
        assert_eq!(out.posts.len(), 1);
        assert!(!out.posts[0].sku_ids.contains(&3));
        assert!(out.shortages.iter().any(|shortage| {
            shortage.kind == "skuImages"
                && shortage.available == 1
                && shortage.detail.contains("SKU 3")
        }));
    }
}
