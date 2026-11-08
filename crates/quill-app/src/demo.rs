//! Sample library for screenshots and first-look demos (`ROUGHCUT_DEMO=1` with an
//! empty library): a handful of documents across collections, three months
//! of writing history, and a whiteboard full of notes.

use jiff::ToSpan;
use quill_doc::{Block, BlockKind, Document, Marks, Pos};
use quill_store::{ItemKind, Library};

fn doc(title: &str, blocks: &[(BlockKind, &str)]) -> Document {
    Document { title: title.into(), blocks: blocks.iter().map(|(k, t)| Block::new(*k, t)).collect() }
}

fn mark(doc: &mut Document, block: usize, needle: &str, mark: Marks) {
    if let Some(start) = doc.blocks[block].text().find(needle) {
        doc.toggle_mark(Pos::new(block, start), Pos::new(block, start + needle.len()), mark);
    }
}

pub fn seed(library: &mut Library) -> quill_store::Result<()> {
    use BlockKind::*;
    let today = quill_store::today();

    let mut lighthouse = doc(
        "The Lighthouse Keeper",
        &[
            (Heading(1), "Chapter One: The Light"),
            (
                Paragraph,
                "The lamp had burned every night for forty-one years, and on the forty-second the keeper let it go dark. Not from neglect — Elias Marr had never neglected anything in his life — but because he wanted, just once, to see what the sea looked like without him watching it.",
            ),
            (
                Paragraph,
                "From the gallery he could make out the long white teeth of the reef, the same reef that had taken the Marguerite and her nine men the winter his father died. The wind came up from the south-west, smelling of rain and kelp, and somewhere below a gull complained about the dark.",
            ),
            (Paragraph, "He told himself he would light it again by midnight. He told himself a great many things."),
            (Heading(2), "The storm arrives"),
            (
                Paragraph,
                "It came the way the old men in the harbour said storms always came to Carrow Point: all at once, and from the wrong direction.",
            ),
        ],
    );
    mark(&mut lighthouse, 5, "all at once, and from the wrong direction.", Marks::BOLD);
    mark(&mut lighthouse, 2, "Marguerite", Marks::ITALIC);

    let docs = [
        (
            Some("Novel"),
            doc(
                "Chapter outline",
                &[
                    (Heading(1), "Outline"),
                    (Numbered, "The Light — Elias lets the lamp go dark."),
                    (Numbered, "Salt — the box from the reef."),
                    (Numbered, "The Log — what his father wrote."),
                    (Numbered, "Carrow Point — the town remembers."),
                ],
            ),
        ),
        (
            Some("Novel"),
            doc(
                "Research: lighthouses",
                &[
                    (Heading(1), "Notes"),
                    (Bullet, "Fresnel lens orders 1–6; first order stands taller than a man."),
                    (Bullet, "Keepers logged the weather every four hours."),
                    (Bullet, "Fog signal: compressed-air horn, two blasts a minute."),
                ],
            ),
        ),
        (
            Some("Letters"),
            doc(
                "Letter to Grandma",
                &[
                    (Paragraph, "Dear Grandma,"),
                    (
                        Paragraph,
                        "Thank you for the scarf. It has been cold here and I wear it every morning on the walk to the station. The man at the bakery asked where I got it and I told him my grandmother knits better than anyone in the county.",
                    ),
                    (Paragraph, "Love, always."),
                ],
            ),
        ),
        (
            Some("Stories"),
            doc(
                "Winter Orchard",
                &[
                    (Heading(1), "Winter Orchard"),
                    (
                        Paragraph,
                        "Nobody picked the last apples that year. They hung on the branches like small red lanterns until the first snow, and then they hung there anyway, stubborn and bright.",
                    ),
                ],
            ),
        ),
        (
            Some("Essays"),
            doc(
                "On Quiet",
                &[
                    (Heading(1), "On Quiet"),
                    (Quote, "There is a kind of silence that only arrives after you have stopped waiting for it."),
                    (
                        Paragraph,
                        "I learned this in the hour before dawn, when the house is still and even the refrigerator seems to be holding its breath.",
                    ),
                ],
            ),
        ),
        (
            Some("Letters"),
            doc(
                "Thank-you notes",
                &[(Paragraph, "To everyone who came to the reading on Saturday — it meant more than I can say.")],
            ),
        ),
    ];
    for (collection, d) in docs.iter().rev() {
        library.create(d, *collection)?;
    }

    // Three months of writing, mostly daily, with a few gaps.
    let pages = library.create(&Document::new("Morning pages"), Some("Journal"))?;
    let mut draft = Document::new("Morning pages");
    for days_ago in (1..=100usize).rev() {
        if days_ago % 9 == 0 || days_ago % 13 == 0 {
            continue;
        }
        let words = 250 + (days_ago * 137 % 1100);
        const LINES: [&str; 6] = [
            "Woke before the alarm again. The kettle, the window, the grey light over the roofs. ",
            "I keep thinking about Elias and why he would let the lamp go dark. ",
            "Wrote a page about my grandfather's boat and then crossed most of it out. ",
            "The trick is to keep the pen moving, even when the sentences are bad. ",
            "Rain all morning. Good for writing, bad for the garden. ",
            "Tried the scene from the gull's point of view. Too clever by half. ",
        ];
        let text: String = (0..words / 14).map(|i| LINES[(i + days_ago) % LINES.len()]).collect();
        let end = draft.end();
        draft.insert_text(end, &text);
        library.commit(pages, &draft, draft.end(), today.checked_sub((days_ago as i64).days()).unwrap_or(today))?;
    }
    let id = library.create(&Document::from_plain_text("The Lighthouse Keeper", ""), Some("Novel"))?;
    let mut partial = lighthouse.clone();
    for keep in 1..lighthouse.blocks.len() {
        partial.blocks = lighthouse.blocks[..keep].to_vec();
        library.commit(id, &partial, partial.end(), today)?;
    }
    library.commit(id, &lighthouse, lighthouse.end(), today)?;

    let left = -(crate::page::TEXT_WIDTH / 2. + 80. + 250.);
    let right = crate::page::TEXT_WIDTH / 2. + 80. + 50.;
    library.add_item(
        id,
        left,
        30.,
        200.,
        150.,
        ItemKind::Text { text: "Elias finds the box in ch. 2, not ch. 3".into(), style: Default::default() },
    )?;
    library.add_item(
        id,
        left + 20.,
        260.,
        190.,
        140.,
        ItemKind::Text { text: "The reef keeps coming back. Guilt?".into(), style: Default::default() },
    )?;
    library.add_item(
        id,
        right,
        60.,
        210.,
        150.,
        ItemKind::Text { text: "First-order Fresnel lens — taller than a man".into(), style: Default::default() },
    )?;
    library.add_item(
        id,
        right + 10.,
        300.,
        200.,
        160.,
        ItemKind::Text { text: "To do\n– name the gull\n– fix the timeline".into(), style: Default::default() },
    )?;
    library.add_item(
        id,
        right,
        520.,
        230.,
        90.,
        ItemKind::Text {
            text: "“The sea doesn’t care who’s watching.” — maybe the last line".into(),
            style: Default::default(),
        },
    )?;
    let arc: Vec<(f32, f32)> = (0..=20)
        .map(|i| {
            let t = i as f32 / 20.;
            (10. + t * t * 60., 6. + t * 74.)
        })
        .collect();
    library.add_item(
        id,
        left + 150.,
        178.,
        110.,
        90.,
        ItemKind::Stroke { points: arc, color: 0x7b9af0, width: 2.5 },
    )?;
    Ok(())
}
