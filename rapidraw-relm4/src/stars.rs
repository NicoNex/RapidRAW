use gtk::prelude::*;
use relm4::prelude::*;

/// A row of 5 clickable stars. Clicking star `n` sets the rating to `n`; clicking the star
/// equal to the current rating clears it to 0 (toggle-off).
pub struct Stars {
    rating: u8,
    buttons: Vec<gtk::Button>,
}

#[derive(Debug)]
pub enum StarsMsg {
    /// User clicked star number `n` (1..=5).
    Clicked(u8),
    /// Programmatic sync (e.g. keyboard 0..5 or loading a new image). Does NOT emit output.
    External(u8),
}

#[derive(Debug)]
pub enum StarsOut {
    Changed(u8),
}

#[relm4::component(pub)]
impl Component for Stars {
    type Init = u8;
    type Input = StarsMsg;
    type Output = StarsOut;
    type CommandOutput = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 0,
            add_css_class: "stars",
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let mut buttons = Vec::with_capacity(5);
        for i in 1..=5u8 {
            let b = gtk::Button::builder().css_classes(["flat", "circular"]).build();
            let s = sender.clone();
            b.connect_clicked(move |_| s.input(StarsMsg::Clicked(i)));
            root.append(&b);
            buttons.push(b);
        }
        let model = Stars { rating: init, buttons };
        model.render();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            StarsMsg::Clicked(n) => {
                self.rating = if self.rating == n { 0 } else { n };
                self.render();
                let _ = sender.output(StarsOut::Changed(self.rating));
            }
            StarsMsg::External(n) => {
                self.rating = n.min(5);
                self.render();
            }
        }
    }
}

impl Stars {
    fn render(&self) {
        for (idx, b) in self.buttons.iter().enumerate() {
            let filled = (idx as u8) < self.rating;
            b.set_label(if filled { "★" } else { "☆" });
        }
    }
}
