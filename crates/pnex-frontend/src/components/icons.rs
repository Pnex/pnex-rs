//! Icônes — SVG inline au style lucide (ISC), `stroke: currentColor` pour
//! hériter de la couleur du texte. Jeu posé d'un bloc (certaines icônes
//! attendent leurs consommateurs — pages suivantes).

#![allow(dead_code)]

use dioxus::prelude::*;

macro_rules! icon {
    ($name:ident, $($d:expr),+ $(,)?) => {
        #[component]
        pub fn $name(class: Option<String>) -> Element {
            rsx! {
                svg {
                    class: class.unwrap_or_default(),
                    xmlns: "http://www.w3.org/2000/svg",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    "stroke-width": "2",
                    "stroke-linecap": "round",
                    "stroke-linejoin": "round",
                    $( path { d: $d } )+
                }
            }
        }
    };
}

// Branding / navigation
icon!(Zap, "M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z");
icon!(
    Home,
    "m3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
    "M9 22V12h6v10"
);
icon!(
    Cpu,
    "M4 4h16a0 0 0 0 1 0 0v16a0 0 0 0 1 0 0H4a0 0 0 0 1 0 0V4a0 0 0 0 1 0 0z",
    "M9 9h6v6H9z",
    "M15 2v2",
    "M15 20v2",
    "M2 15h2",
    "M2 9h2",
    "M20 15h2",
    "M20 9h2",
    "M9 2v2",
    "M9 20v2"
);
icon!(Package,
    "M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z",
    "m3.3 7 8.7 5 8.7-5", "M12 22V12");
icon!(Wrench, "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z");
icon!(Building,
    "M4 22h16a2 2 0 0 0 2-2V4a2 2 0 0 0-2-2H8a2 2 0 0 0-2 2v16a2 2 0 0 1-2 2Zm0 0a2 2 0 0 1-2-2v-9c0-1.1.9-2 2-2h2",
    "M18 14h-8", "M15 18h-5", "M10 6h8v4h-8V6Z");
icon!(
    User,
    "M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2",
    "M12 3a4 4 0 1 0 0 8 4 4 0 0 0 0-8"
);

// Interface
icon!(Menu, "M4 6h16", "M4 12h16", "M4 18h16");
icon!(X, "M18 6 6 18", "M6 6l12 12");
icon!(ChevronDown, "m6 9 6 6 6-6");
icon!(Plus, "M5 12h14", "M12 5v14");
icon!(
    Search,
    "M11 3a8 8 0 1 0 0 16 8 8 0 0 0 0-16",
    "m21 21-4.3-4.3"
);
icon!(
    Trash2,
    "M3 6h18",
    "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6",
    "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2",
    "M10 11v6",
    "M14 11v6"
);
icon!(Check, "M20 6 9 17l-5-5");
icon!(
    LogOut,
    "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4",
    "m16 17 5-5-5-5",
    "M21 12H9"
);
icon!(
    RefreshCw,
    "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8",
    "M21 3v5h-5",
    "M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16",
    "M8 16H3v5"
);
icon!(
    Download,
    "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4",
    "m7 10 5 5 5-5",
    "M12 15V3"
);
icon!(
    BookOpen,
    "M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z",
    "M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"
);
icon!(
    ShoppingCart,
    "M6 6h15l-1.58 7.42a2 2 0 0 1-1.96 1.58H8.4a2 2 0 0 1-1.96-1.58L4.5 3H2",
    "M9 20a1 1 0 1 0 0 2 1 1 0 0 0 0-2",
    "M18 20a1 1 0 1 0 0 2 1 1 0 0 0 0-2"
);

// Statuts
icon!(
    CheckCircle,
    "M22 11.08V12a10 10 0 1 1-5.93-9.14",
    "m9 11 3 3L22 4"
);
icon!(
    AlertTriangle,
    "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z",
    "M12 9v4",
    "M12 17h.01"
);
icon!(
    Info,
    "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20",
    "M12 16v-4",
    "M12 8h.01"
);
icon!(Key, "m21 2-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0 3 3L22 7l-3-3m-3.5 3.5L19 4");
icon!(
    Wifi,
    "M5 13a10 10 0 0 1 14 0",
    "M8.5 16.5a5 5 0 0 1 7 0",
    "M2 8.82a15 15 0 0 1 20 0",
    "M12 20h.01"
);
icon!(LineChart, "M3 3v18h18", "m19 9-5 5-4-4-3 3");
icon!(
    Workflow,
    "M15 13H19A2 2 0 0 1 21 15V19A2 2 0 0 1 19 21H15A2 2 0 0 1 13 19V15A2 2 0 0 1 15 13Z",
    "M7 11v4a2 2 0 0 0 2 2h4",
    "M5 3H9A2 2 0 0 1 11 5V9A2 2 0 0 1 9 11H5A2 2 0 0 1 3 9V5A2 2 0 0 1 5 3Z"
);
