use axum::http::{header, HeaderMap};

macro_rules! translations {
    ($($key:ident => $en:literal, $fr:literal;)*) => {
        pub(crate) struct Texts { $(pub(crate) $key: &'static str,)* }
        const EN: Texts = Texts { $($key: $en,)* };
        const FR: Texts = Texts { $($key: $fr,)* };
    };
}

translations! {
    lang => "en", "fr";
    product => "Pensine", "Pensine";
    guide => "Connect an assistant", "Connecter un assistant";
    assistant => "Which assistant?", "Quel assistant ?";
    claude => "Claude on the web, app or phone", "Claude sur le web, dans l’application ou sur téléphone";
    chatgpt => "ChatGPT", "ChatGPT";
    other => "Another assistant with a key", "Un autre assistant avec une clé";
    reach => "What can it reach?", "Que peut-il atteindre ?";
    reach_help => "Choose here: all your spaces, or only some. The consent page will show your choice. You can change it later.", "Vous choisissez ici : tous vos espaces, ou seulement certains. La page de consentement qui s’ouvre affiche votre choix. Vous pourrez le changer plus tard.";
    all => "All your spaces", "Tous vos espaces";
    selected => "Only selected spaces", "Seulement certains espaces";
    children => "Selecting a space includes its subspaces.", "Si vous cochez un espace, ses sous-espaces sont inclus.";
    read_only => "Read only", "Lecture seule";
    read_write => "Read and write", "Lecture et écriture";
    in_assistant => "In the assistant", "Dans l’assistant";
    public_access => "This assistant calls your Pensine node over the internet: the address must open from outside your network.", "Cet assistant appelle votre nœud Pensine depuis internet : l’adresse doit s’ouvrir hors de votre réseau.";
    claude_setup => "In Claude (web, app or phone): Settings, then Connectors, then Add custom connector.", "Dans Claude (web, application ou téléphone) : Réglages, puis Connecteurs, puis Ajouter un connecteur personnalisé.";
    chatgpt_setup => "In ChatGPT, enable developer mode and create a custom MCP app.", "Dans ChatGPT, activez le mode développeur et créez une application MCP personnalisée.";
    key_setup => "Use the key as a bearer token.", "Utilisez la clé comme jeton Bearer.";
    paste => "Paste this address", "Collez cette adresse";
    address => "MCP address", "Adresse MCP";
    no_public => "Turn on Tailscale in the add-on options, then reload", "Activez Tailscale dans les options du module complémentaire, puis rechargez la page";
    copy => "Copy", "Copier";
    copied => "Copied", "Copié";
    create_code => "Create a code", "Créer un code";
    enter_code => "Enter the one-time code when asked.", "Saisissez le code à usage unique quand il est demandé.";
    code_expiry => "This one-time code expires in 10 minutes.", "Ce code à usage unique expire dans 10 minutes.";
    select_space => "Select at least one space.", "Cochez au moins un espace.";
    connections => "Connections and keys", "Connexions et clés";
    sessions => "Assistants signed in with a code", "Assistants connectés avec un code";
    new_rights => "To change rights, revoke the session and create a new code.", "Pour changer les droits, révoquez la session puis créez un nouveau code.";
    spaces => "Spaces", "Espaces";
    status => "Status", "État";
    notes => "notes", "notes";
    files => "files", "fichiers";
    disk => "Disk used", "Espace disque utilisé";
    bytes => "bytes", "octets";
    alias => "Alias", "Alias";
    save_alias => "Save alias", "Enregistrer l’alias";
    invalid_alias => "This alias is reserved, taken or invalid.", "Cet alias est réservé, déjà utilisé ou invalide.";
    name => "Name", "Nom";
    name_required => "A connection name is required.", "Indiquez un nom pour la connexion.";
    create_key => "Create a key", "Créer une clé";
    save_key => "Copy this key now. It is shown only once.", "Copiez cette clé maintenant. Elle ne s’affiche qu’une fois.";
    revoke => "Revoke", "Révoquer";
    expires => "Days until expiry", "Jours avant expiration";
    consent_title => "Pensine consent", "Autorisation Pensine";
    connect_to => "Connect to", "Connecter à";
    one_time_code => "One-time code", "Code à usage unique";
    next => "Continue", "Continuer";
    choose_for => "Choose spaces for", "Choisissez les espaces pour";
    rights_for => "Access for", "Accès pour";
    allow => "Allow", "Autoriser";
    deny => "Deny", "Refuser";
}

pub(crate) fn for_request(headers: &HeaderMap) -> &'static Texts {
    let first = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(',')
        .next()
        .unwrap_or("")
        .trim_start();
    if first
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("fr"))
    {
        &FR
    } else {
        &EN
    }
}
