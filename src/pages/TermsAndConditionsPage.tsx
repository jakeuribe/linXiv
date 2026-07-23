// Single-file React terms & conditions page, converted from "LinXIV T&Cs.odt".
// Reviewer/editorial comments in the source were dropped — not part of the terms.

type ArticleItem = [term: string, body: string, list?: string[]];

type Article = {
  title: string;
  items: ArticleItem[];
};

const ARTICLES: Article[] = [
  {
    title: "Article 1 — Definitions",
    items: [
      ["Acceptance.", '"Acceptance" means your first download, installation, access to, or use of the Software, whichever occurs first.'],
      ["Derivative Work.", '"Derivative Work" means any work based upon the Software or any portion thereof, including any modification, enhancement, translation, abridgment, condensation, expansion, or any other form in which the Software may be recast, transformed, or adapted.'],
      ["Documentation.", '"Documentation" means any user manuals, technical documentation, README files, installation guides, and other materials provided with or related to the Software.'],
      ["Intellectual Property Rights.", '"Intellectual Property Rights" means all patents, copyrights, trademarks, trade secrets, and any other proprietary or intellectual property rights recognized under the laws of the United States, the State of Minnesota, or any other jurisdiction.'],
      ["Open Source License.", '"Open Source License" means the General Public License v3.0 under which the Software is distributed, a copy of which is included with the Software and is incorporated into these Terms by reference. The technology enabling the feature known as the "Shared" page is located at "https://github.com/linxiv-dev/linxiv-p2p" and is licensed under the Apache License 2.0; which is strictly more permissive than the General Public License v3.0'],
      ["Personal Data.", '"Personal Data" means any information relating to an identified or identifiable natural person as defined under applicable data protection laws.'],
      ["Source Code.", '"Source Code" means the human-readable form of the Software, including all modules, interfaces, and scripts, together with any associated build scripts, configuration files, and compilation instructions.'],
    ],
  },
  {
    title: "Article 2 — License Grant and Scope",
    items: [
      ["Grant of License.", "Subject to your compliance with these Terms and the Open Source License, we hereby grant you a worldwide, royalty-free, non-exclusive, perpetual (subject to termination as provided herein) license to use, copy, modify, merge, publish, distribute, and sublicense the Software and Documentation, in Source Code or object code form, and to create Derivative Works, all in accordance with the terms of the Open Source License."],
      ["Open Source License Governs.", "The rights and obligations set forth in the Open Source License shall control in the event of any conflict between these Terms and the Open Source License with respect to the scope of the license grant, permitted uses, modification rights, and redistribution requirements. You agree to comply fully with all terms and conditions of the Open Source License."],
      ["No Charge.", "The Software is made available to you at no charge. You are not required to pay any license fee, subscription fee, or other monetary consideration to download, use, or distribute the Software, except that you remain responsible for any costs associated with internet connectivity, hardware, or third-party services required to use the Software."],
      ["Attribution and Notices.", "You agree to retain all copyright notices, license notices, disclaimers, and attributions contained in the Software and Documentation. If you distribute the Software or any Derivative Work, you must include a copy of these Terms, the Open Source License, and all required notices in a conspicuous location."],
      ["No Trademark Rights.", "Except as expressly permitted by the Open Source License, nothing in these Terms grants you any right to use our trademarks, service marks, trade names, logos, or other brand features. Any use of our trademarks must comply with our separate trademark usage guidelines, if any."],
    ],
  },
  {
    title: "Article 3 — Restrictions and Prohibited Uses",
    items: [
      ["Compliance with Law.", "You agree to use the Software only in compliance with all applicable federal, state, and local laws and regulations, including but not limited to laws governing data protection, export control, intellectual property, and computer fraud."],
      ["Prohibited Activities.", "You shall not:", [
        "Use the Software for any unlawful purpose or in any manner that violates these Terms.",
        "Reverse engineer, decompile, or disassemble the Software, except to the extent such restriction is expressly prohibited by applicable law or the Open Source License.",
        "Remove, alter, or obscure any copyright, trademark, or other proprietary notices contained in the Software or Documentation.",
        "Use the Software to develop or distribute malware, viruses, or other malicious code.",
        "Use the Software in any manner that could damage, disable, overburden, or impair any server, network, or system.",
        "Use the Software to violate the privacy, intellectual property rights, or other rights of any third party.",
      ]],
      ["Export Control.", "You acknowledge that the Software may be subject to export control laws and regulations of the United States. You agree not to export, re-export, or transfer the Software, directly or indirectly, to any country, entity, or person prohibited by such laws or regulations without obtaining any required government authorization."],
      ["No Warranty Void.", "Any attempt to use the Software in a manner inconsistent with these Terms or the Open Source License may result in automatic termination of your rights under these Terms and may void any implied warranties to the maximum extent permitted by law."],
    ],
  },
  {
    title: "Article 4 — Intellectual Property Ownership",
    items: [
      ["Ownership of Software.", "All right, title, and interest in and to the Software, including all Intellectual Property Rights therein, are and shall remain the exclusive property of Lin Xiv and its licensors. These Terms do not convey to you any ownership interest in the Software, but only a limited right to use the Software in accordance with these Terms and the Open Source License."],
      ["Contributions.", 'If you submit, provide, or otherwise make available to us any suggestions, comments, feedback, enhancements, modifications, or other contributions related to the Software ("Contributions"), you hereby grant us a worldwide, royalty-free, fully paid-up, perpetual, irrevocable, non-exclusive, transferable, and sublicensable license to use, reproduce, modify, distribute, publicly display, publicly perform, and otherwise exploit such Contributions for any purpose, including incorporation into the Software or Derivative Works, without any obligation of attribution, compensation, or accounting to you.'],
      ["Derivative Works Ownership.", "You retain all Intellectual Property Rights in any Derivative Works that you create, subject to our underlying rights in the Software and subject to the terms of the Open Source License. If you distribute any Derivative Work, you must do so under the terms of the Open Source License and these Terms."],
      ["Third-Party Components.", "The Software may include or be distributed with third-party open source software components, each governed by its own license terms. You agree to comply with all such third-party license terms. A list of third-party components and their applicable licenses, if any, is included in the Documentation or a NOTICES file distributed with the Software."],
    ],
  },
  {
    title: "Article 5 — Warranties and Disclaimers",
    items: [
      ["AS-IS Basis.", 'THE SOFTWARE AND DOCUMENTATION ARE PROVIDED "AS IS" AND "AS AVAILABLE," WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED. TO THE MAXIMUM EXTENT PERMITTED BY APPLICABLE LAW, WE DISCLAIM ALL WARRANTIES, INCLUDING BUT NOT LIMITED TO IMPLIED WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, TITLE, NON-INFRINGEMENT, ACCURACY, COMPLETENESS, QUIET ENJOYMENT, AND ANY WARRANTIES ARISING OUT OF COURSE OF DEALING OR USAGE OF TRADE.'],
      ["No Guarantee of Performance.", "WE DO NOT WARRANT THAT THE SOFTWARE WILL MEET YOUR REQUIREMENTS, OPERATE WITHOUT INTERRUPTION, BE ERROR-FREE, SECURE, OR FREE FROM VIRUSES OR OTHER HARMFUL COMPONENTS. WE DO NOT WARRANT THAT DEFECTS WILL BE CORRECTED OR THAT THE SOFTWARE WILL BE COMPATIBLE WITH YOUR HARDWARE, SOFTWARE, OR SYSTEMS."],
      ["No Support Obligation.", "We are under no obligation to provide support, maintenance, updates, upgrades, bug fixes, or any other services related to the Software. Any support or maintenance that we choose to provide shall be at our sole discretion and may be discontinued at any time without notice."],
      ["Your Responsibility.", "You acknowledge and agree that you assume full responsibility for your use of the Software and for any consequences arising from such use. You are solely responsible for determining the suitability of the Software for your purposes and for implementing appropriate safeguards, including data backups and security measures."],
      ["Minnesota Law.", "Some jurisdictions do not allow the exclusion of implied warranties, so the above exclusions may not apply to you to the extent prohibited by the laws of the State of Minnesota or other applicable law. In such case, the duration of any implied warranties shall be limited to the maximum extent permitted by law."],
    ],
  },
  {
    title: "Article 6 — Limitation of Liability",
    items: [
      ["Exclusion of Damages.", "TO THE MAXIMUM EXTENT PERMITTED BY APPLICABLE LAW, IN NO EVENT SHALL WE, OUR AFFILIATES, OFFICERS, DIRECTORS, EMPLOYEES, AGENTS, LICENSORS, OR SUPPLIERS BE LIABLE TO YOU OR ANY THIRD PARTY FOR ANY INDIRECT, INCIDENTAL, SPECIAL, CONSEQUENTIAL, PUNITIVE, OR EXEMPLARY DAMAGES, INCLUDING BUT NOT LIMITED TO DAMAGES FOR LOSS OF PROFITS, GOODWILL, USE, DATA, OR OTHER INTANGIBLE LOSSES, ARISING OUT OF OR RELATED TO YOUR USE OF OR INABILITY TO USE THE SOFTWARE, EVEN IF WE HAVE BEEN ADVISED OF THE POSSIBILITY OF SUCH DAMAGES."],
      ["Cap on Direct Damages.", "TO THE MAXIMUM EXTENT PERMITTED BY APPLICABLE LAW, OUR TOTAL AGGREGATE LIABILITY TO YOU FOR ANY AND ALL CLAIMS ARISING OUT OF OR RELATED TO THESE TERMS OR YOUR USE OF THE SOFTWARE, WHETHER IN CONTRACT, TORT (INCLUDING NEGLIGENCE), STRICT LIABILITY, OR OTHERWISE, SHALL NOT EXCEED FIFTY DOLLARS ($50.00)."],
      ["Basis of the Bargain.", "YOU ACKNOWLEDGE AND AGREE THAT THE DISCLAIMERS AND LIMITATIONS SET FORTH IN THIS ARTICLE 6 REFLECT A REASONABLE AND FAIR ALLOCATION OF RISK BETWEEN YOU AND US, AND THAT THESE LIMITATIONS ARE AN ESSENTIAL BASIS OF OUR DECISION TO MAKE THE SOFTWARE AVAILABLE TO YOU AT NO CHARGE. THESE LIMITATIONS SHALL APPLY NOTWITHSTANDING THE FAILURE OF ESSENTIAL PURPOSE OF ANY LIMITED REMEDY."],
      ["Jurisdictional Limitations.", "Some jurisdictions do not allow the limitation or exclusion of liability for incidental or consequential damages, so the above limitations may not apply to you to the extent prohibited by the laws of the State of Minnesota or other applicable law. In such case, our liability shall be limited to the maximum extent permitted by law."],
    ],
  },
  {
    title: "Article 7 — Indemnification",
    items: [
      ["Your Indemnity Obligation.", "You agree to indemnify, defend, and hold harmless us, our affiliates, and our respective officers, directors, employees, agents, licensors, and suppliers from and against any and all claims, liabilities, damages, losses, costs, expenses, or fees (including reasonable attorneys' fees) arising out of or related to:", [
        "Your use or misuse of the Software.",
        "Your violation of these Terms or the Open Source License.",
        "Your violation of any applicable law or regulation.",
        "Your violation of any third-party right, including any Intellectual Property Right, privacy right, or publicity right.",
        "Any Contribution you provide to us.",
      ]],
      ["Control of Defense.", "We reserve the right, at our own expense, to assume the exclusive defense and control of any matter otherwise subject to indemnification by you, in which event you will cooperate with us in asserting any available defenses."],
    ],
  },
  {
    title: "Article 8 — Data Privacy and Security",
    items: [
      ["No Collection of Personal Data.", "The Software, as distributed by us, does not collect, transmit, or store any Personal Data. However, you acknowledge that your use of the Software, including any modifications you make or third-party components you integrate, may involve the collection, processing, or storage of Personal Data."],
      ["Your Privacy Obligations.", "If you use the Software in a manner that involves the collection, processing, or storage of Personal Data, you are solely responsible for complying with all applicable data protection and privacy laws, including but not limited to the Minnesota Government Data Practices Act (Minn. Stat. Ch. 13), the Health Insurance Portability and Accountability Act (HIPAA), the California Consumer Privacy Act (CCPA), and the General Data Protection Regulation (GDPR) if applicable. You agree to implement appropriate technical and organizational measures to protect any Personal Data."],
      ["No Liability for Data Breaches.", "We shall not be liable for any data breach, unauthorized access, loss of data, or other security incident arising out of your use of the Software. You acknowledge that the Software is provided without any security warranties and that you assume all risk related to data security."],
      ["Third-Party Analytics.", "If you modify the Software to include third-party analytics, tracking, or monitoring tools, you are solely responsible for complying with all applicable laws regarding notice, consent, and data protection with respect to such tools."],
    ],
  },
  {
    title: "Article 9 — Termination",
    items: [
      ["Termination by You.", "You may terminate these Terms at any time by ceasing all use of the Software and destroying all copies of the Software in your possession or control."],
      ["Termination by Us.", "We reserve the right to terminate or suspend your rights under these Terms immediately, without notice, if you breach any provision of these Terms or the Open Source License, or if we believe that your use of the Software poses a risk to us, other users, or third parties."],
      ["Effect of Termination.", "Upon termination of these Terms for any reason:", [
        "All rights granted to you under these Terms shall immediately cease.",
        "You must promptly cease all use of the Software and destroy all copies of the Software and Documentation in your possession or control.",
        "Any provisions of these Terms that by their nature should survive termination shall survive, including but not limited to Articles 4 (Intellectual Property Ownership), 5 (Warranties and Disclaimers), 6 (Limitation of Liability), 7 (Indemnification), and 10 (Miscellaneous).",
      ]],
      ["No Refund.", "Because the Software is provided at no charge, no refund or compensation of any kind shall be due upon termination."],
    ],
  },
  {
    title: "Article 10 — Miscellaneous",
    items: [
      ["Governing Law.", "These Terms and any disputes arising out of or related to these Terms or your use of the Software shall be governed by and construed in accordance with the laws of the State of Minnesota, without regard to its conflict of law principles."],
      ["Jurisdiction and Venue.", "You agree that any action or proceeding arising out of or related to these Terms or your use of the Software shall be brought exclusively in the state or federal courts located in Hennepin County, Minnesota, and you hereby consent to the personal jurisdiction and venue of such courts."],
      ["Entire Agreement.", "These Terms, together with the Open Source License incorporated by reference, constitute the entire agreement between you and us regarding the Software and supersede all prior or contemporaneous understandings and agreements, whether written or oral, regarding the subject matter hereof."],
      ["Amendment.", "We reserve the right to modify these Terms at any time by posting revised Terms on our website or within the Software repository. Your continued use of the Software after any such modification constitutes your acceptance of the revised Terms. It is your responsibility to review these Terms periodically for changes."],
      ["Waiver.", "No waiver of any provision of these Terms shall be deemed or shall constitute a waiver of any other provision, nor shall any waiver constitute a continuing waiver. No waiver shall be binding unless executed in writing by the party making the waiver."],
      ["Severability.", "If any provision of these Terms is held to be invalid, illegal, or unenforceable by a court of competent jurisdiction, the validity, legality, and enforceability of the remaining provisions shall not be affected or impaired thereby, and such provision shall be reformed to the minimum extent necessary to make it enforceable while preserving the intent of the parties."],
      ["Assignment.", "You may not assign, transfer, or delegate these Terms or any rights or obligations hereunder, in whole or in part, without our prior written consent. Any attempted assignment in violation of this section shall be void. We may assign these Terms at any time without notice to you. Subject to the foregoing, these Terms shall bind and inure to the benefit of the parties and their respective successors and permitted assigns."],
      ["No Third-Party Beneficiaries.", "These Terms are for the sole benefit of you and us and do not create any third-party beneficiary rights in any other person or entity."],
      ["Force Majeure.", "Neither party shall be liable for any failure or delay in performance under these Terms (except for payment obligations) due to causes beyond its reasonable control, including but not limited to acts of God, natural disasters, war, terrorism, labor disputes, government actions, or failures of the internet or telecommunications infrastructure."],
      ["Notices.", "Any notice required or permitted under these Terms shall be provided to us at the address or email provided in the Software repository or Documentation. Notices to you may be provided by email to the address you provide, by posting on our website, or by posting within the Software repository. Notices shall be deemed given upon receipt or, if posted, upon posting."],
      ["Relationship of Parties.", "The parties are independent contractors. These Terms do not create any partnership, joint venture, agency, franchise, employment, or fiduciary relationship between you and us."],
      ["Headings.", "The headings and captions in these Terms are for convenience only and shall not affect the interpretation of these Terms."],
      ["Counterparts.", "These Terms may be accepted electronically or in counterparts, each of which shall be deemed an original and all of which together shall constitute one and the same instrument."],
    ],
  },
];

export default function TermsAndConditionsPage() {
  return (
    <main style={{ maxWidth: 800, margin: "0 auto", padding: "2rem 1.25rem", lineHeight: 1.6, fontFamily: "system-ui, sans-serif", color: "#1a1a1a" }}>
      <h1 style={{ fontSize: "1.8rem", marginBottom: ".25rem" }}>
        Open Source Software Terms and Conditions
      </h1>
      <p style={{ color: "#666", margin: 0 }}>Effective Date: 7/18/2026</p>

      <p style={{ marginTop: "1.5rem" }}>
        These Terms and Conditions (these "Terms") govern your access to and use of linXiv
        (the "Software"), an open source software application made available by Robuck Del Toro Labs,
        linXiv development("we," "us," or "our"). By downloading, installing, accessing, or using the
        Software, you ("you" or "User") agree to be bound by these Terms. If you do not agree
        to these Terms, do not use the Software.
      </p>

      {ARTICLES.map((a) => (
        <section key={a.title} style={{ marginTop: "2rem" }}>
          <h2 style={{ fontSize: "1.25rem", borderBottom: "1px solid #eee", paddingBottom: ".3rem" }}>
            {a.title}
          </h2>
          {a.items.map(([term, body, list], i) => (
            <p key={i} style={{ marginTop: "1rem" }}>
              <strong>{term}</strong> {body}
              {list && (
                <ul style={{ marginTop: ".5rem" }}>
                  {list.map((li, j) => <li key={j} style={{ marginBottom: ".35rem" }}>{li}</li>)}
                </ul>
              )}
            </p>
          ))}
        </section>
      ))}
    </main>
  );
}
